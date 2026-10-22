import { describe, it, expect, vi, beforeEach } from "vitest";
import { act, render } from "@testing-library/react";
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
    /** Each mounted editor's model, in mount order. */
    models: [] as unknown[],
    /** Gives the last mounted editor a new model, as a changed `path` does; returns it. */
    swapModel: undefined as (() => unknown) | undefined,
    /**
     * Every `registerCompletionItemProvider`, as Monaco keeps them: per LANGUAGE,
     * not per editor. `disposed` turns true when the component disposes it.
     */
    providers: [] as {
      language: string;
      provide: (model: unknown, position: unknown) => { suggestions: { label: string }[] };
      disposed: boolean;
    }[],
    /** The `value` the last render passed to `@monaco-editor/react` (it syncs a defined one into the editor). */
    passedValue: undefined as string | undefined,
    /** The mounted editor's text. */
    text: "",
    /** Every write of the caller's text into the editor. */
    writes: [] as string[],
    /** Types into the mounted editor: its text changes and `onChange` fires, as Monaco does. */
    type: undefined as ((text: string) => void) | undefined,
    /** Calls the mounted component's `onMount` again, as `@monaco-editor/react` can. */
    remount: undefined as (() => void) | undefined,
    reset() {
      this.commands = {};
      this.actions = [];
      this.providers = [];
      this.models = [];
      this.writes = [];
    },
    runCommand(): (() => void) | undefined {
      return this.commands[KeyMod.CtrlCmd | KeyCode.Enter];
    },
  };
});

vi.mock("@/components/monaco/MonacoEditor", () => ({
  default: function MonacoStub({
    value,
    defaultValue,
    onChange,
    onMount,
    beforeMount,
    options,
  }: {
    value?: string;
    defaultValue?: string;
    onChange?: (v: string | undefined) => void;
    onMount?: (editor: unknown, monaco: unknown) => void;
    beforeMount?: (monaco: unknown) => void;
    options?: { ariaLabel?: string; readOnly?: boolean };
  }) {
    monacoStub.passedValue = value;
    const latest = useRef(value ?? defaultValue ?? "");
    if (value !== undefined) latest.current = value;
    const onChangeRef = useRef(onChange);
    onChangeRef.current = onChange;
    useEffect(() => {
      const monaco = {
        KeyMod: monacoStub.KeyMod,
        KeyCode: monacoStub.KeyCode,
        editor: { defineTheme: () => {} },
        languages: {
          CompletionItemKind: {},
          registerCompletionItemProvider: (
            language: string,
            provider: { provideCompletionItems: (model: unknown, position: unknown) => { suggestions: { label: string }[] } },
          ) => {
            const entry = { language, provide: provider.provideCompletionItems, disposed: false };
            monacoStub.providers.push(entry);
            return { dispose: () => { entry.disposed = true; } };
          },
        },
      };
      beforeMount?.(monaco);
      const editorId = `editor-${++monacoStub.editorSeq}`;
      // Each editor owns its model, as `@monaco-editor/react` gives it one.
      let model = {
        editorId,
        getValue: () => latest.current,
        getValueInRange: () => latest.current,
        getWordUntilPosition: () => ({ word: "", startColumn: 1, endColumn: 1 }),
        getFullModelRange: () => ({ startLineNumber: 1, startColumn: 1, endLineNumber: 1, endColumn: 1 }),
      };
      monacoStub.models.push(model);
      monacoStub.text = latest.current;
      monacoStub.type = (t: string) => {
        latest.current = t;
        monacoStub.text = t;
        onChangeRef.current?.(t);
      };
      const editor = {
        getId: () => editorId,
        getValue: () => latest.current,
        setValue: (t: string) => { latest.current = t; monacoStub.text = t; monacoStub.writes.push(t); },
        executeEdits: (_src: string, edits: { text: string }[]) => {
          latest.current = edits[0].text;
          monacoStub.text = latest.current;
          monacoStub.writes.push(latest.current);
          return true;
        },
        pushUndoStop: () => true,
        getSelection: () => null,
        getModel: () => model,
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
      monacoStub.remount = () => onMount?.(editor, monaco);
      monacoStub.swapModel = () => {
        model = { ...model };
        return model;
      };
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

  it("disposes the first mount's action when onMount runs a second time", () => {
    render(<SqlEditor value="SELECT 1" onChange={() => {}} schemaWords={[]} onRun={() => {}} />);
    expect(monacoStub.actions).toHaveLength(1);

    act(() => monacoStub.remount!());

    expect(monacoStub.actions).toHaveLength(2);
    expect(monacoStub.actions[0].disposed).toBe(true);
    expect(monacoStub.actions[1].disposed).toBe(false);
  });
});

/**
 * Completion providers are kept per LANGUAGE (`registerCompletionItemProvider`),
 * and Monaco asks every provider of one score and merges the answers
 * (`suggest.js` `provideSuggestionItems`). So two mounted `SqlEditor`s would each
 * answer for BOTH models: each editor would offer the other's schema, and every
 * keyword twice. `suggest` asks the live `sql` providers as Monaco does.
 */
describe("SqlEditor — completion is scoped to its editor", () => {
  beforeEach(() => monacoStub.reset());

  const schemaOf = (table: string) => ({ identifiers: [table], tables: [table], columnsByTable: {} });
  const modelOf = (editorId: string) => ({
    editorId,
    getValue: () => "",
    getValueInRange: () => "",
    getWordUntilPosition: () => ({ word: "", startColumn: 1, endColumn: 1 }),
  });
  const position = { lineNumber: 1, column: 1 };
  const suggest = (model: unknown) =>
    monacoStub.providers
      .filter((p) => p.language === "sql" && !p.disposed)
      .flatMap((p) => p.provide(model, position).suggestions.map((s) => s.label));

  it("offers each editor only its own schema, and each keyword once", () => {
    render(
      <>
        <SqlEditor value="" onChange={() => {}} schemaWords={["only_first"]} schema={schemaOf("lake.sales")} />
        <SqlEditor value="" onChange={() => {}} schemaWords={["only_second"]} schema={schemaOf("lake.users")} />
      </>,
    );
    expect(monacoStub.providers.filter((p) => !p.disposed)).toHaveLength(2);
    // Asked through the editors' OWN model objects, as Monaco asks.
    const [firstModel, secondModel] = monacoStub.models;
    const first = suggest(firstModel);
    const second = suggest(secondModel);

    expect(first).toContain("lake.sales");
    expect(first).toContain("only_first");
    expect(first).not.toContain("lake.users");
    expect(first).not.toContain("only_second");
    expect(second).toContain("lake.users");
    expect(second).not.toContain("lake.sales");
    expect(first.filter((l) => l === "SELECT")).toHaveLength(1);
    expect(second.filter((l) => l === "SELECT")).toHaveLength(1);

    // A model no SqlEditor owns (same shape, another object) gets nothing from them.
    expect(suggest(modelOf("someone-else"))).toEqual([]);
  });

  it("still answers after the editor's model is swapped, and not for the old one", () => {
    render(<SqlEditor value="" onChange={() => {}} schemaWords={[]} schema={schemaOf("lake.sales")} />);
    const [oldModel] = monacoStub.models;
    const newModel = monacoStub.swapModel!();

    expect(suggest(newModel)).toContain("lake.sales");
    expect(suggest(oldModel)).toEqual([]);
  });

  it("disposes its provider when it unmounts", () => {
    const { unmount } = render(<SqlEditor value="" onChange={() => {}} schemaWords={[]} />);
    expect(monacoStub.providers).toHaveLength(1);
    expect(monacoStub.providers[0].disposed).toBe(false);

    unmount();
    expect(monacoStub.providers[0].disposed).toBe(true);
  });
});

/**
 * KI-006. `@monaco-editor/react` writes every `value` it is given into the
 * editor when it differs from the editor's text (`executeEdits` over the whole
 * text). SQL Lab's value makes a round trip through tab state, so it could
 * arrive one keystroke old while the editor already held the next one, and the
 * write erased what was just typed. Measured in the browser: typing at 40 ms a
 * key with a 1,000-row result on screen lost characters.
 */
describe("SqlEditor — typing is never overwritten by the caller's echo", () => {
  beforeEach(() => monacoStub.reset());

  function Parent({ value }: { value: string }) {
    return <SqlEditor value={value} onChange={() => {}} schemaWords={[]} />;
  }

  it("does not hand the editor its own text back, even one keystroke late", () => {
    const { rerender } = render(<Parent value="" />);
    // Monaco is not driven by a `value`: nothing for it to write back.
    expect(monacoStub.passedValue).toBeUndefined();
    act(() => { monacoStub.type!("S"); monacoStub.type!("SE"); });
    // The caller renders with the first keystroke's text: one keystroke behind.
    rerender(<Parent value="S" />);
    rerender(<Parent value="SE" />);
    expect(monacoStub.text).toBe("SE");
    expect(monacoStub.writes).toEqual([]);
  });

  it("still takes a change from outside (Format, a snippet, a history entry)", () => {
    const { rerender } = render(<Parent value="select 1" />);
    act(() => monacoStub.type!("select 1 "));
    rerender(<Parent value="select 1 " />);
    rerender(<Parent value={"SELECT\n  1"} />);
    expect(monacoStub.text).toBe("SELECT\n  1");
    expect(monacoStub.writes).toEqual(["SELECT\n  1"]);
  });
});
