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
    /**
     * Every `addAction`, with the id of the editor that added it. Monaco scopes an
     * action's keybindings to its own editor (`editorId == getId()`); `disposed`
     * turns true when the component disposes it. The stub editor has NO
     * `addCommand`: a shortcut registered through it would answer in every editor
     * and never be removed, so a regression throws here.
     */
    actions: [] as { editorId: string; id: string; keybindings: number[]; run: () => void; disposed: boolean }[],
    editorSeq: 0,
    reset() {
      this.commands = {};
      this.actions = [];
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
      const editorId = `editor-${++monacoStub.editorSeq}`;
      const editor = {
        getId: () => editorId,
        getValue: () => latest.current,
        getSelection: () => null,
        getModel: () => null,
        addAction: (desc: { id: string; label: string; keybindings?: number[]; run: (e: unknown) => unknown }) => {
          const entry = {
            editorId,
            id: desc.id,
            keybindings: desc.keybindings ?? [],
            run: () => void desc.run(editor),
            disposed: false,
          };
          monacoStub.actions.push(entry);
          for (const k of entry.keybindings) monacoStub.commands[k] = entry.run;
          return { dispose: () => { entry.disposed = true; } };
        },
        onDidChangeCursorSelection: () => ({ dispose: () => {} }),
      };
      onMount?.(editor, monaco);
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

/**
 * ⌘↵ is an editor ACTION, scoped to its editor. Standalone Monaco keeps every
 * editor's `addCommand` keybinding in one shared service: it answers in whichever
 * editor has focus, the newest registration wins (`keybindingResolver.js`
 * `_findCommand`), and it is never removed — so with two SQL Lab tabs, ⌘↵ in the
 * first ran the second's query, even after the second had closed.
 */
describe("SqlEditor — ⌘↵ is scoped to its editor", () => {
  beforeEach(() => monacoStub.reset());

  it("binds ⌘↵ as an action of its own editor, and each editor's action runs only its own handler", () => {
    const first = vi.fn();
    const second = vi.fn();
    render(
      <>
        <SqlEditor value="SELECT 1" onChange={() => {}} schemaWords={[]} onRun={first} />
        <SqlEditor value="SELECT 2" onChange={() => {}} schemaWords={[]} onRun={second} />
      </>,
    );

    const runKey = monacoStub.KeyMod.CtrlCmd | monacoStub.KeyCode.Enter;
    expect(monacoStub.actions).toHaveLength(2);
    const [a, b] = monacoStub.actions;
    expect(a.editorId).not.toBe(b.editorId);
    expect(a.keybindings).toEqual([runKey]);
    expect(b.keybindings).toEqual([runKey]);

    a.run();
    expect(first).toHaveBeenCalledWith("SELECT 1");
    expect(second).not.toHaveBeenCalled();
    b.run();
    expect(second).toHaveBeenCalledWith("SELECT 2");
    expect(first).toHaveBeenCalledTimes(1);
  });

  it("disposes its action when it unmounts", () => {
    const { unmount } = render(<SqlEditor value="SELECT 1" onChange={() => {}} schemaWords={[]} onRun={() => {}} />);
    expect(monacoStub.actions).toHaveLength(1);
    expect(monacoStub.actions[0].disposed).toBe(false);

    unmount();
    expect(monacoStub.actions[0].disposed).toBe(true);
  });
});
