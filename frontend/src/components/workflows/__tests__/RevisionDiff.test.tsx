import { afterEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
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
  props: [] as DiffEditorProps[],
}));
vi.mock("@/components/monaco/MonacoEditor", () => ({
  MonacoDiffEditor: function FakeDiff(props: DiffEditorProps) {
    fake.props.push(props);
    useEffect(() => {
      props.onMount?.({ getModel: () => ({ original: fake.original, modified: fake.modified }) } as never, {} as never);
      // Mounted once, as the real editor is.
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);
    return <div data-testid="fake-diff" />;
  },
}));

import { RevisionDiff } from "@/components/workflows/RevisionDiff";

/** `SIDE_BY_SIDE_MIN_PX` in `RevisionDiff.tsx`, spelled here so the test also runs against a component without it. */
const INLINE_BELOW_PX = 600;

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  fake.props.length = 0;
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

  it("goes inline below its breakpoint, and the captions stop claiming two columns", () => {
    // The width a ResizeObserver reports; jsdom has none and lays nothing out.
    let width = 0;
    let notify: (() => void) | undefined;
    vi.stubGlobal("ResizeObserver", class {
      constructor(private cb: ResizeObserverCallback) {}
      observe() {
        notify = () => this.cb([{ contentRect: { width } } as ResizeObserverEntry], this as never);
        notify();
      }
      disconnect() {}
    });
    width = INLINE_BELOW_PX - 1;
    render(<RevisionDiff left={{ label: "v1", source: "a: 1\n" }} right={{ label: "Current draft", source: "a: 2\n" }} />);
    expect(screen.getByTestId("revision-diff-labels")).toHaveTextContent("v1 → Current draft − only in v1, + only in Current draft");
    expect(fake.props.at(-1)?.options).toMatchObject({ renderSideBySide: false, useInlineViewWhenSpaceIsLimited: false });

    width = INLINE_BELOW_PX;
    act(() => notify!());
    expect(screen.getByTestId("revision-diff-labels")).toHaveTextContent(/^v1Current draft$/);
    expect(fake.props.at(-1)?.options).toMatchObject({ renderSideBySide: true, useInlineViewWhenSpaceIsLimited: false });
  });
});
