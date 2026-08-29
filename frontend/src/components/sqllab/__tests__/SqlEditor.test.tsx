import { describe, it, expect, vi, beforeEach } from "vitest";
import { render } from "@testing-library/react";
import { useEffect, useRef } from "react";

/**
 * The same minimal fake Monaco the three consumers' suites use
 * (`Query.test.tsx`, `EndpointCreate.test.tsx`,
 * `components/notebook/__tests__/CellEditor.test.tsx`). Real Monaco cannot load
 * in jsdom, and `onMount` here runs `SqlEditor`'s REAL `handleMount`, so the
 * command captured below is the one the component actually registers.
 */
const monacoStub = vi.hoisted(() => {
  const KeyMod = { CtrlCmd: 1 << 11 };
  const KeyCode = { Enter: 3 };
  return {
    KeyMod,
    KeyCode,
    commands: {} as Record<number, () => void>,
    reset() {
      this.commands = {};
    },
    runCommand(): (() => void) | undefined {
      return this.commands[KeyMod.CtrlCmd | KeyCode.Enter];
    },
  };
});

vi.mock("@/components/monaco/MonacoEditor", () => ({
  default: function MonacoStub({
    value,
    onMount,
    beforeMount,
    options,
  }: {
    value: string;
    onMount?: (editor: unknown, monaco: unknown) => void;
    beforeMount?: (monaco: unknown) => void;
    options?: { ariaLabel?: string; readOnly?: boolean };
  }) {
    const latest = useRef(value);
    latest.current = value;
    useEffect(() => {
      const monaco = {
        KeyMod: monacoStub.KeyMod,
        KeyCode: monacoStub.KeyCode,
        editor: { defineTheme: () => {} },
        languages: {
          CompletionItemKind: {},
          registerCompletionItemProvider: () => ({ dispose: () => {} }),
        },
      };
      beforeMount?.(monaco);
      onMount?.(
        {
          getValue: () => latest.current,
          getSelection: () => null,
          getModel: () => null,
          addCommand: (binding: number, cb: () => void) => {
            monacoStub.commands[binding] = cb;
          },
          onDidChangeCursorSelection: () => ({ dispose: () => {} }),
        },
        monaco,
      );
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);
    return <textarea aria-label={options?.ariaLabel} readOnly={!!options?.readOnly} value={value} />;
  },
}));

import { SqlEditor } from "@/components/sqllab/SqlEditor";

/**
 * ⌘↵ and `readOnly`, which the component used to leave entirely to its callers.
 *
 * **A read-only Monaco still fires its commands.** `readOnly`/`domReadOnly` stop
 * TYPING; they do not unregister a keybinding, and a surrounding
 * `<fieldset disabled>` never sees a keystroke either — so the only thing
 * standing between a read-only editor and its `onRun` was every call site
 * remembering to pass `undefined`. `Query.tsx` does remember (`onRun={trashed ?
 * undefined : …}`, pinned by its own test), so nothing was reachable in the
 * product; what was wrong is that the signature could express the unsafe
 * configuration at all, and the next caller to pass both would get a keyboard
 * path around its own read-only surface with nothing to tell it.
 */
describe("SqlEditor — ⌘↵ under readOnly", () => {
  beforeEach(() => monacoStub.reset());

  it("does not run a read-only editor from the keyboard, even when given an onRun", () => {
    const onRun = vi.fn();
    render(<SqlEditor value="SELECT 1" onChange={() => {}} schemaWords={[]} readOnly onRun={onRun} />);

    // Registered, not absent: the component always binds ⌘↵, which is exactly
    // why the guard has to live inside the callback.
    const run = monacoStub.runCommand();
    expect(run).toBeTypeOf("function");
    run!();
    expect(onRun).not.toHaveBeenCalled();
  });

  it("still runs a writable editor from the keyboard", () => {
    // The other half, and it is not ceremony: an editor whose ⌘↵ never fired
    // would pass the test above and break SQL Lab, whose Run shortcut is the
    // main way that page is driven.
    const onRun = vi.fn();
    render(<SqlEditor value="SELECT 1" onChange={() => {}} schemaWords={[]} onRun={onRun} />);

    const run = monacoStub.runCommand();
    expect(run).toBeTypeOf("function");
    run!();
    expect(onRun).toHaveBeenCalledTimes(1);
    expect(onRun).toHaveBeenCalledWith("SELECT 1");
  });
});
