import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const validationCalls: unknown[][] = [];
vi.mock("@/components/workflows/editor/useServerValidation", () => ({
  useServerValidation: (...a: unknown[]) => {
    validationCalls.push(a);
    return { status: "idle", findings: [], error: null, retry: () => {} };
  },
}));
vi.mock("@/components/workflows/editor/useFlowSchema", () => ({
  useFlowSchema: () => ({ status: "loading", schema: null, etag: null, error: null, retry: () => {} }),
}));

import { createEditorBridge, useFlowEditor } from "@/components/workflows/editor/EditorContext";
import { disposeFlowModel, flowModelPath, rememberMonaco } from "@/components/workflows/editor/flowModel";

beforeEach(() => { validationCalls.length = 0; });
afterEach(() => vi.useRealTimers());

describe("useFlowEditor", () => {
  it("holds the source, names the model after the flow, and validates what is typed", () => {
    const { result } = renderHook(() =>
      useFlowEditor({ workspace: "ws1", flowId: "f-1", initialSource: "id: a", readOnly: false }),
    );
    expect(result.current.modelKey).toBe("f-1");
    expect(result.current.source).toBe("id: a");
    act(() => result.current.setSource("id: b"));
    expect(result.current.source).toBe("id: b");
    expect(validationCalls.at(-1)).toEqual(["ws1", "id: b", "f-1", { enabled: true }]);
  });

  it("on the create page the model is 'new' and validation has no id", () => {
    const { result } = renderHook(() => useFlowEditor({ workspace: "ws1", initialSource: "", readOnly: false }));
    expect(result.current.modelKey).toBe("new");
    expect(validationCalls.at(-1)).toEqual(["ws1", "", undefined, { enabled: true }]);
  });

  it("does not ask the server to validate for a read-only viewer", () => {
    renderHook(() => useFlowEditor({ workspace: "ws1", flowId: "f-1", initialSource: "", readOnly: true }));
    expect(validationCalls.at(-1)?.[3]).toEqual({ enabled: false });
  });

  it("passes the cursor on 150 ms after it stops moving", () => {
    vi.useFakeTimers();
    const { result } = renderHook(() => useFlowEditor({ workspace: "ws1", initialSource: "", readOnly: false }));
    act(() => result.current.setCursorOffset(3));
    act(() => { vi.advanceTimersByTime(100); });
    act(() => result.current.setCursorOffset(7));
    act(() => { vi.advanceTimersByTime(149); });
    expect(result.current.cursorOffset).toBeNull();
    act(() => { vi.advanceTimersByTime(1); });
    expect(result.current.cursorOffset).toBe(7);
  });

  it("focusRange asks the page to show the Code tab and moves the editor", () => {
    const onRevealCode = vi.fn();
    const { result } = renderHook(() =>
      useFlowEditor({ workspace: "ws1", initialSource: "", readOnly: false, onRevealCode }),
    );
    const reveal = vi.fn();
    result.current.bridge.attach({ reveal });
    const range = { startLine: 2, startCol: 1, endLine: 2, endCol: 4 };
    act(() => result.current.focusRange(range));
    expect(onRevealCode).toHaveBeenCalledTimes(1);
    expect(reveal).toHaveBeenCalledWith(range);
  });
});

describe("createEditorBridge", () => {
  it("keeps a range asked for while no editor is attached, and applies it once", () => {
    const bridge = createEditorBridge();
    const range = { startLine: 1, startCol: 1, endLine: 1, endCol: 2 };
    bridge.focus(range);
    const first = { reveal: vi.fn() };
    bridge.attach(first);
    expect(first.reveal).toHaveBeenCalledWith(range);
    bridge.detach(first);
    const second = { reveal: vi.fn() };
    bridge.attach(second);
    expect(second.reveal).not.toHaveBeenCalled();
  });

  it("a detach by an editor that was already replaced does not drop the new one", () => {
    const bridge = createEditorBridge();
    const old = { reveal: vi.fn() };
    const next = { reveal: vi.fn() };
    bridge.attach(old);
    bridge.attach(next);
    bridge.detach(old);
    bridge.focus({ startLine: 1, startCol: 1, endLine: 1, endCol: 2 });
    expect(next.reveal).toHaveBeenCalledTimes(1);
    expect(old.reveal).not.toHaveBeenCalled();
  });
});

describe("disposeFlowModel", () => {
  it("disposes the flow's model, and only it", () => {
    const models = new Map<string, { dispose: ReturnType<typeof vi.fn> }>([
      [flowModelPath("f-1"), { dispose: vi.fn() }],
      [flowModelPath("f-2"), { dispose: vi.fn() }],
    ]);
    rememberMonaco({
      Uri: { parse: (s: string) => s },
      editor: { getModel: (uri: string) => models.get(uri) ?? null },
    } as never);
    disposeFlowModel("f-1");
    expect(models.get(flowModelPath("f-1"))!.dispose).toHaveBeenCalledTimes(1);
    expect(models.get(flowModelPath("f-2"))!.dispose).not.toHaveBeenCalled();
    expect(() => disposeFlowModel("missing")).not.toThrow();
  });

  it("names every flow's model under the glob monaco-yaml attaches the schema to", () => {
    expect(flowModelPath("0b6f…")).toMatch(/^inmemory:\/\/egret\/flow\/[^/]+\.yaml$/);
    expect(flowModelPath("new")).toBe("inmemory://egret/flow/new.yaml");
  });
});
