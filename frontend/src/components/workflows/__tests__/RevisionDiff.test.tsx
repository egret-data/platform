import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { useEffect } from "react";
import type { DiffEditorProps } from "@monaco-editor/react";

/**
 * A fake diff editor that mounts the way `@monaco-editor/react` does: it hands
 * `onMount` an editor whose `getModel()` returns the two models, which record
 * their `dispose` calls.
 */
const fake = vi.hoisted(() => ({
  original: { dispose: vi.fn() },
  modified: { dispose: vi.fn() },
}));
vi.mock("@/components/monaco/MonacoEditor", () => ({
  MonacoDiffEditor: function FakeDiff(props: DiffEditorProps) {
    useEffect(() => {
      props.onMount?.({ getModel: () => ({ original: fake.original, modified: fake.modified }) } as never, {} as never);
      // Mounted once, as the real editor is.
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);
    return <div data-testid="fake-diff" />;
  },
}));

import { RevisionDiff } from "@/components/workflows/RevisionDiff";

afterEach(() => {
  vi.useRealTimers();
  fake.original.dispose.mockReset();
  fake.modified.dispose.mockReset();
});

describe("RevisionDiff", () => {
  it("disposes both models after unmount, one task later, and not before", () => {
    vi.useFakeTimers();
    const view = render(<RevisionDiff left={{ label: "v1", source: "a: 1\n" }} right={{ label: "v2", source: "a: 2\n" }} />);
    expect(screen.getByTestId("fake-diff")).toBeInTheDocument();
    view.rerender(<RevisionDiff left={{ label: "v1", source: "a: 1\n" }} right={{ label: "v3", source: "a: 3\n" }} />);
    // A new pair reuses the models: nothing is disposed while mounted.
    expect(fake.original.dispose).not.toHaveBeenCalled();
    view.unmount();
    // Deferred, so that the library has disposed the widget first.
    expect(fake.original.dispose).not.toHaveBeenCalled();
    expect(fake.modified.dispose).not.toHaveBeenCalled();
    vi.runAllTimers();
    expect(fake.original.dispose).toHaveBeenCalledTimes(1);
    expect(fake.modified.dispose).toHaveBeenCalledTimes(1);
  });

  it("says when the two sides have the same text", () => {
    const { rerender } = render(<RevisionDiff left={{ label: "v1", source: "a: 1\n" }} right={{ label: "Current draft", source: "a: 1\n" }} />);
    expect(screen.getByRole("status")).toHaveTextContent("v1 and Current draft have the same text.");
    rerender(<RevisionDiff left={{ label: "v1", source: "a: 1\n" }} right={{ label: "Current draft", source: "a: 2\n" }} />);
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });
});
