import { useState } from "react";
import { act, render, renderHook, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), info: vi.fn() } }));
import { DiscardFileDialog, nameList, useFilesHost } from "@/components/workflows/editor/fileTabs";
import type { NamespaceFile } from "@/components/workflows/namespaces/files/useNamespaceFile";
import { CODE_TAB, closeTab, type Layout } from "@/components/workflows/panels/layout";

/**
 * The page's side of the file tabs, on its own: a close that drops several file tabs with
 * unsaved edits names every one, and a confirmed close is applied to the layout as it is when
 * confirmed (W7d Task 7, fix round 1).
 */
const START: Layout = {
  panels: [
    { id: "p1", size: 50, tabs: [CODE_TAB, "file:a.py"], active: "file:a.py" },
    { id: "p2", size: 50, tabs: ["file:b.py", "file:c.py"], active: "file:b.py" },
  ],
};

const session = (dirty: boolean) => ({ dirty }) as unknown as NamespaceFile;

function useHarness() {
  const [layout, setLayout] = useState(START);
  const files = useFilesHost({ ws: "eng", namespace: "company.team", readOnly: false, flowLayout: { layout, setLayout, revealCode: () => {} } });
  return { layout, files };
}

describe("useFilesHost: closing file tabs with unsaved edits", () => {
  it("names every dropped file with unsaved edits, and applies the close to the current layout", () => {
    const { result } = renderHook(useHarness);
    act(() => {
      result.current.files.host.store.set("/a.py", session(true));
      result.current.files.host.store.set("/b.py", session(true));
      result.current.files.host.store.set("/c.py", session(false));
    });
    // One change dropping all three file tabs (a frame's close drops one; the host must still name all).
    const dropAll = ["file:a.py", "file:b.py", "file:c.py"].reduce((l, t) => closeTab(l, t), START);
    act(() => result.current.files.flowLayout.setLayout(dropAll));
    expect(result.current.files.closing).toEqual(["/a.py", "/b.py"]);
    expect(result.current.layout).toBe(START);

    // Meanwhile another file opens: the confirmed close must not undo it.
    act(() => result.current.files.host.open("/d.py"));
    act(() => result.current.files.confirmClose());
    expect(result.current.files.closing).toBeNull();
    expect(result.current.layout.panels.flatMap((p) => p.tabs)).toEqual([CODE_TAB, "file:d.py"]);
  });

  it("closes at once when no dropped tab has unsaved edits", () => {
    const { result } = renderHook(useHarness);
    act(() => result.current.files.host.store.set("/a.py", session(false)));
    act(() => result.current.files.flowLayout.setLayout(closeTab(START, "file:a.py")));
    expect(result.current.files.closing).toBeNull();
    expect(result.current.layout.panels[0].tabs).toEqual([CODE_TAB]);
  });

  it("Keep editing drops the question and changes nothing", () => {
    const { result } = renderHook(useHarness);
    act(() => result.current.files.host.store.set("/a.py", session(true)));
    act(() => result.current.files.flowLayout.setLayout(closeTab(START, "file:a.py")));
    act(() => result.current.files.cancelClose());
    expect(result.current.files.closing).toBeNull();
    expect(result.current.layout).toBe(START);
  });
});

describe("DiscardFileDialog", () => {
  it("names one file, or every file", () => {
    const { rerender } = render(<DiscardFileDialog paths={["/a.py"]} onConfirm={() => {}} onCancel={() => {}} />);
    expect(screen.getByRole("alertdialog", { name: "Close a.py without saving?" })).toHaveTextContent("It has unsaved edits.");
    rerender(<DiscardFileDialog paths={["/a.py", "/s/b.py", "/c.py"]} onConfirm={() => {}} onCancel={() => {}} />);
    expect(screen.getByRole("alertdialog", { name: "Close a.py, b.py and c.py without saving?" })).toHaveTextContent("They have unsaved edits.");
  });

  it("nameList joins names in English", () => {
    expect([nameList([]), nameList(["a"]), nameList(["a", "b"]), nameList(["a", "b", "c"])]).toEqual(["", "a", "a and b", "a, b and c"]);
  });
});
