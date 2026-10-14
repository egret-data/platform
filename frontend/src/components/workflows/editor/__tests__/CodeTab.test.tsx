import { useEffect, useRef, useState } from "react";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { EditorProps } from "@monaco-editor/react";

/**
 * A fake Monaco. Real Monaco cannot load in jsdom; this one records what the
 * Code tab hands it: the props, the markers it sets per owner, the commands it
 * binds, and how often the editor mounts (a remount is a second mount).
 *
 * Its models live by path and outlive the editor, as Monaco's do under
 * `keepCurrentModel`; a model is created from `defaultValue` only when its
 * path has none. Typing goes to the model and is then reported through
 * `onChange`, as Monaco's does.
 */
const fake = vi.hoisted(() => {
  const MarkerSeverity = { Hint: 1, Info: 2, Warning: 4, Error: 8 };
  const KeyMod = { CtrlCmd: 1 << 11 };
  const KeyCode = { KeyS: 49 };
  type FakeModel = {
    uri: { toString: () => string };
    text: string;
    listeners: Set<() => void>;
    pushes: string[];
    getValue: () => string;
    getFullModelRange: () => unknown;
    pushStackElement: () => void;
    pushEditOperations: (_s: unknown, ops: { text: string }[]) => null;
  };
  const state = {
    mounts: 0,
    props: null as null | Record<string, unknown>,
    models: new Map<string, FakeModel>(),
    markers: new Map<string, unknown[]>(),
    markerListeners: [] as Array<() => void>,
    commands: {} as Record<number, () => void>,
    /** Each command's `when` context key, by keybinding (W7d Task 7, fix round 1). */
    commandContexts: {} as Record<number, string | undefined>,
    /** The editor context keys created. */
    contextKeys: [] as string[],
    /** Every `addAction`: its id, keybindings, and whether it was disposed (W7d Task 7, fix round 2). */
    actions: [] as { id: string; keybindings: number[]; disposed: boolean }[],
    selection: null as unknown,
    revealed: null as unknown,
    focused: 0,
    yamlMarkers: [] as unknown[],
  };
  function modelFor(path: string, initial: string): FakeModel {
    let m = state.models.get(path);
    if (!m) {
      const model: FakeModel = {
        uri: { toString: () => path },
        text: initial,
        listeners: new Set(),
        pushes: [],
        getValue: () => model.text,
        getFullModelRange: () => "all",
        pushStackElement: () => {},
        pushEditOperations: (_s, ops) => {
          model.text = ops[0].text;
          model.pushes.push(ops[0].text);
          model.listeners.forEach((l) => l());
          return null;
        },
      };
      m = model;
      state.models.set(path, m);
    }
    return m;
  }
  const monaco = {
    MarkerSeverity,
    KeyMod,
    KeyCode,
    editor: {
      setModelMarkers: (_m: unknown, owner: string, markers: unknown[]) => {
        state.markers.set(owner, markers);
        // Monaco fires this for any owner's markers, the Code tab's own included.
        state.markerListeners.forEach((l) => l());
      },
      getModelMarkers: () => state.yamlMarkers,
      onDidChangeMarkers: (cb: () => void) => {
        state.markerListeners.push(cb);
        return { dispose: () => { state.markerListeners = state.markerListeners.filter((l) => l !== cb); } };
      },
    },
  };
  return {
    state,
    modelFor,
    monaco,
    reset() {
      Object.assign(state, {
        mounts: 0, props: null, models: new Map(), markers: new Map(), markerListeners: [], commands: {}, commandContexts: {}, contextKeys: [], actions: [],
        selection: null, revealed: null, focused: 0, yamlMarkers: [],
      });
    },
  };
});

vi.mock("@/components/monaco/MonacoEditor", () => ({
  default: function MonacoStub(props: EditorProps) {
    fake.state.props = props as Record<string, unknown>;
    const model = fake.modelFor(props.path ?? "default", props.defaultValue ?? "");
    const [, repaint] = useState(0);
    const latest = useRef(props);
    useEffect(() => { latest.current = props; });
    useEffect(() => {
      fake.state.mounts++;
      const changed = () => {
        repaint((n) => n + 1);
        latest.current.onChange?.(model.text, undefined as never);
      };
      model.listeners.add(changed);
      const editor = {
        getModel: () => model,
        addCommand: (k: number, cb: () => void, context?: string) => {
          fake.state.commands[k] = cb;
          fake.state.commandContexts[k] = context;
        },
        addAction: (desc: { id: string; keybindings?: number[]; run: () => unknown }) => {
          const entry = { id: desc.id, keybindings: desc.keybindings ?? [], disposed: false };
          fake.state.actions.push(entry);
          // Monaco runs the action on its keybinding: tests press it through `commands`, as before.
          for (const k of entry.keybindings) fake.state.commands[k] = () => void desc.run();
          return { dispose: () => { entry.disposed = true; } };
        },
        createContextKey: (key: string, value: unknown) => {
          fake.state.contextKeys.push(key);
          return { set: () => {}, get: () => value, reset: () => {} };
        },
        onDidChangeCursorPosition: () => ({ dispose: () => {} }),
        getPosition: () => null,
        setSelection: (r: unknown) => { fake.state.selection = r; },
        revealRangeInCenter: (r: unknown) => { fake.state.revealed = r; },
        focus: () => { fake.state.focused++; },
      };
      latest.current.beforeMount?.(fake.monaco as never);
      latest.current.onMount?.(editor as never, fake.monaco as never);
      return () => { model.listeners.delete(changed); };
    }, [model]);
    return (
      <textarea
        aria-label="Flow source"
        readOnly={!!props.options?.readOnly}
        value={model.text}
        onChange={(e) => {
          model.text = e.target.value;
          model.listeners.forEach((l) => l());
        }}
      />
    );
  },
}));

const configureFlowYaml = vi.fn();
vi.mock("@/components/workflows/editor/monacoYaml", () => ({
  configureFlowYaml: (...a: unknown[]) => configureFlowYaml(...a),
}));

import { CodeTab, FLOW_SAVE_ACTION } from "@/components/workflows/editor/CodeTab";
import { registerEditorTabs } from "@/components/workflows/editor/registerEditorTabs";
import { createEditorBridge, type EditorContext } from "@/components/workflows/editor/EditorContext";
import { MultiPanel } from "@/components/workflows/panels/MultiPanel";
import { DEFAULT_FLOW_LAYOUT, type Layout } from "@/components/workflows/panels/layout";
import { makeEditorContext } from "./fixtures";

beforeEach(() => {
  fake.reset();
  configureFlowYaml.mockReset();
});

describe("CodeTab", () => {
  it("edits one stable model per flow and keeps it, with its view state, across a remount", () => {
    render(<CodeTab ctx={makeEditorContext({ modelKey: "f-1", source: "id: a" })} />);
    const p = fake.state.props!;
    expect(p.path).toBe("inmemory://egret/flow/f-1.yaml");
    expect(p.keepCurrentModel).toBe(true);
    expect(p.saveViewState).toBe(true);
    expect(p.defaultLanguage).toBe("yaml");
    // Uncontrolled: a `value` prop would race the keyboard (see CodeTab).
    expect(p.value).toBeUndefined();
    expect(p.defaultValue).toBe("id: a");
  });

  it("keeps the source when the panel frame remounts it (a split)", async () => {
    registerEditorTabs();
    function Page() {
      const [layout, setLayout] = useState<Layout>(DEFAULT_FLOW_LAYOUT);
      const [source, setSource] = useState("id: a");
      const ctx: EditorContext = makeEditorContext({ modelKey: "f-1", source, setSource });
      return <div style={{ height: 400 }}><MultiPanel<EditorContext> layout={layout} onChange={setLayout} context={ctx} /></div>;
    }
    const user = userEvent.setup();
    render(<Page />);
    const box = screen.getByRole("textbox", { name: "Flow source" });
    // A change event, not a typed one: in jsdom react-resizable-panels cancels every
    // pointer press (zero rects put it on a gutter), so a click cannot focus the box.
    fireEvent.change(box, { target: { value: "id: edited" } });
    expect(box).toHaveValue("id: edited");
    expect(fake.state.mounts).toBe(1);

    // Splitting Docs off changes the set of panels, which remounts every panel, Code's too.
    screen.getByRole("button", { name: "Tab actions: Docs" }).focus();
    await user.keyboard("{Enter}");
    await user.click(await screen.findByRole("menuitem", { name: "Split right" }));

    await waitFor(() => expect(screen.getAllByRole("tablist")).toHaveLength(3));
    expect(fake.state.mounts).toBe(2);
    expect(screen.getByRole("textbox", { name: "Flow source" })).toHaveValue("id: edited");
    expect(fake.state.props!.path).toBe("inmemory://egret/flow/f-1.yaml");
    // Had the model been lost, Monaco would rebuild it from this, which the context kept.
    expect(fake.state.props!.defaultValue).toBe("id: edited");
  });

  it("writes a source changed from outside into the model as one edit, and ignores late echoes of typing", () => {
    const setSource = vi.fn();
    const view = (source: string) => <CodeTab ctx={makeEditorContext({ modelKey: "f-1", source, setSource })} />;
    const { rerender } = render(view("id: a"));
    const box = screen.getByRole("textbox", { name: "Flow source" });
    const model = fake.state.models.get("inmemory://egret/flow/f-1.yaml")!;

    fireEvent.change(box, { target: { value: "id: a1" } });
    fireEvent.change(box, { target: { value: "id: a12" } });
    expect(setSource.mock.calls.map((c) => c[0])).toEqual(["id: a1", "id: a12"]);
    // The render for the first key arrives after the second key: it must not undo it.
    rerender(view("id: a1"));
    expect(model.pushes).toEqual([]);
    expect(box).toHaveValue("id: a12");
    rerender(view("id: a12"));
    expect(model.pushes).toEqual([]);

    // A reload or a revision replaces the text: the editor follows.
    rerender(view("id: reloaded"));
    expect(model.pushes).toEqual(["id: reloaded"]);
    expect(box).toHaveValue("id: reloaded");
  });

  it("sets server findings as markers with the owner egret-validate, at their mapped ranges", () => {
    render(<CodeTab ctx={makeEditorContext({
      modelKey: "f-1",
      source: "id: a\nnamespace: b\n",
      validation: {
        status: "ok", error: null, retry: vi.fn(),
        findings: [
          { severity: "warning", code: "w", message: "odd namespace", path: "namespace" },
          { severity: "error", code: "e", message: "no such thing", path: "tasks.ghost" },
        ],
      },
    })} />);
    expect(fake.state.markers.get("egret-validate")).toEqual([
      { severity: 8, message: "no such thing (tasks.ghost)", startLineNumber: 1, startColumn: 1, endLineNumber: 1, endColumn: 6 },
      { severity: 4, message: "odd namespace", startLineNumber: 2, startColumn: 1, endLineNumber: 2, endColumn: 13 },
    ]);
  });

  it("reports the schema's own markers to the context, and not its own", () => {
    const setSchemaMarkers = vi.fn();
    render(<CodeTab ctx={makeEditorContext({ modelKey: "f-1", source: "id: a", setSchemaMarkers })} />);
    fake.state.yamlMarkers = [
      { owner: "yaml", severity: 8, message: "Missing property \"tasks\".", startLineNumber: 1, startColumn: 1, endLineNumber: 1, endColumn: 3 },
      { owner: "yaml", severity: 1, message: "a hint", startLineNumber: 1, startColumn: 1, endLineNumber: 1, endColumn: 2 },
      { owner: "egret-validate", severity: 8, message: "ours", startLineNumber: 1, startColumn: 1, endLineNumber: 1, endColumn: 2 },
    ];
    act(() => fake.state.markerListeners.forEach((l) => l()));
    // The same markers again (setting its own fires the event too) are not reported twice,
    // or reporting and re-rendering would chase each other.
    act(() => fake.state.markerListeners.forEach((l) => l()));
    expect(setSchemaMarkers).toHaveBeenCalledTimes(2);
    expect(setSchemaMarkers).toHaveBeenLastCalledWith([
      { severity: "error", message: "Missing property \"tasks\".", range: { startLine: 1, startCol: 1, endLine: 1, endCol: 3 } },
    ]);
  });

  it("configures monaco-yaml with the schema and its ETag", async () => {
    const schema = { type: "object" };
    render(<CodeTab ctx={makeEditorContext({
      workspace: "ws1",
      modelKey: "f-1",
      schema: { status: "ready", schema, etag: '"e1"', error: null, retry: vi.fn() },
    })} />);
    await waitFor(() => expect(configureFlowYaml).toHaveBeenCalled());
    expect(configureFlowYaml).toHaveBeenLastCalledWith(fake.monaco, { schema, etag: '"e1"', workspace: "ws1" });
  });

  it("attaches to the bridge so the Problems tab can move its cursor", () => {
    const bridge = createEditorBridge();
    render(<CodeTab ctx={makeEditorContext({ modelKey: "f-1", source: "id: a", bridge })} />);
    bridge.focus({ startLine: 1, startCol: 1, endLine: 1, endCol: 3 });
    const r = { startLineNumber: 1, startColumn: 1, endLineNumber: 1, endColumn: 3 };
    expect(fake.state.selection).toEqual(r);
    expect(fake.state.revealed).toEqual(r);
    expect(fake.state.focused).toBe(1);
  });

  it("applies a range asked for before the editor mounted", () => {
    const bridge = createEditorBridge();
    bridge.focus({ startLine: 2, startCol: 1, endLine: 2, endCol: 4 });
    render(<CodeTab ctx={makeEditorContext({ modelKey: "f-1", source: "id: a\nx: 1", bridge })} />);
    expect(fake.state.selection).toEqual({ startLineNumber: 2, startColumn: 1, endLineNumber: 2, endColumn: 4 });
  });

  it("saves on Ctrl/Cmd+S", () => {
    const save = vi.fn();
    render(<CodeTab ctx={makeEditorContext({ modelKey: "f-1", save })} />);
    fake.state.commands[fake.monaco.KeyMod.CtrlCmd | fake.monaco.KeyCode.KeyS]();
    expect(save).toHaveBeenCalledTimes(1);
  });

  it("binds Ctrl/Cmd+S as an action of its own editor, not a shared command, and disposes it on unmount", () => {
    // An `addCommand` keybinding answers in every editor and is never removed; an action's is
    // scoped to the editor that added it (W7d Task 7, fix round 2).
    const save = vi.fn();
    const { unmount } = render(<CodeTab ctx={makeEditorContext({ modelKey: "f-1", save })} />);
    const saveKey = fake.monaco.KeyMod.CtrlCmd | fake.monaco.KeyCode.KeyS;
    expect(fake.state.actions).toEqual([{ id: FLOW_SAVE_ACTION, keybindings: [saveKey], disposed: false }]);
    expect(saveKey in fake.state.commandContexts).toBe(false);
    unmount();
    expect(fake.state.actions[0].disposed).toBe(true);
  });

  it("is read-only with a banner that says why for a viewer without workflow:manage", () => {
    render(<CodeTab ctx={makeEditorContext({ modelKey: "f-1", readOnly: true })} />);
    expect(screen.getByRole("note")).toHaveTextContent("Read only. Editing a flow needs the workflow:manage permission.");
    const opts = fake.state.props!.options as { readOnly: boolean; domReadOnly: boolean };
    expect(opts.readOnly).toBe(true);
    expect(opts.domReadOnly).toBe(true);
  });

  it("does not save from the keyboard when read-only", () => {
    const save = vi.fn();
    render(<CodeTab ctx={makeEditorContext({ modelKey: "f-1", readOnly: true, save })} />);
    fake.state.commands[fake.monaco.KeyMod.CtrlCmd | fake.monaco.KeyCode.KeyS]();
    expect(save).not.toHaveBeenCalled();
  });

  it("says autocomplete is unavailable, with a retry, when the schema failed", async () => {
    const retry = vi.fn();
    render(<CodeTab ctx={makeEditorContext({
      modelKey: "f-1",
      schema: { status: "error", schema: null, etag: null, error: "internal error", retry },
    })} />);
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("Autocomplete unavailable");
    await userEvent.click(within(alert).getByRole("button", { name: "Retry loading the schema" }));
    expect(retry).toHaveBeenCalledTimes(1);
  });
});
