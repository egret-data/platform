import { useEffect, useRef, useState } from "react";
import type { DiffEditorProps, EditorProps } from "@monaco-editor/react";

/**
 * A stand-in for `@/components/monaco/MonacoEditor`, for the flow pages'
 * tests: real Monaco cannot load in jsdom. Mock the module with
 * `vi.mock("@/components/monaco/MonacoEditor", () => import("./monacoStub"))`.
 *
 * It keeps what the Code tab relies on: one model per `path` that outlives the
 * editor (`keepCurrentModel`), created from `defaultValue` only when the path
 * has none; typing goes to the model and is reported through `onChange`; a
 * replacement from outside goes through `pushEditOperations`. `Ctrl/Cmd+S`
 * inside the editor is the command the tab binds, run by {@link pressSaveInEditor}.
 */
type Model = {
  uri: { toString: () => string };
  text: string;
  disposed: boolean;
  listeners: Set<() => void>;
  getValue: () => string;
  getFullModelRange: () => unknown;
  pushStackElement: () => void;
  pushEditOperations: (_s: unknown, ops: { text: string }[]) => null;
  dispose: () => void;
};

export const stub = {
  models: new Map<string, Model>(),
  commands: new Map<number, () => void>(),
  /** The props of every diff editor rendered, latest last. */
  diffs: [] as DiffEditorProps[],
  /** The path of every model disposed, in order. */
  disposed: [] as string[],
  /** The editors mounted now, each showing one model (`monaco.editor.getEditors()`). */
  editors: new Set<{ getModel: () => Model }>(),
};

export function resetMonacoStub(): void {
  stub.models.clear();
  stub.commands.clear();
  stub.diffs = [];
  stub.disposed = [];
  stub.editors.clear();
}

/** Run the editor's own `Ctrl/Cmd+S` binding, as Monaco would on that key. */
export function pressSaveInEditor(): void {
  stub.commands.get(KeyMod.CtrlCmd | KeyCode.KeyS)?.();
}

const KeyMod = { CtrlCmd: 1 << 11 };
const KeyCode = { KeyS: 49 };

function modelFor(path: string, initial: string): Model {
  const existing = stub.models.get(path);
  if (existing && !existing.disposed) return existing;
  const model: Model = {
    uri: { toString: () => path },
    text: initial,
    disposed: false,
    listeners: new Set(),
    getValue: () => model.text,
    getFullModelRange: () => "all",
    pushStackElement: () => {},
    pushEditOperations: (_s, ops) => {
      model.text = ops[0].text;
      model.listeners.forEach((l) => l());
      return null;
    },
    dispose: () => {
      model.disposed = true;
      stub.models.delete(path);
      stub.disposed.push(path);
    },
  };
  stub.models.set(path, model);
  return model;
}

/** Typing, as Monaco reports it: the model changes, then its listeners run. */
function typeInto(model: Model, text: string): void {
  model.text = text;
  model.listeners.forEach((l) => l());
}

const monaco = {
  MarkerSeverity: { Hint: 1, Info: 2, Warning: 4, Error: 8 },
  KeyMod,
  KeyCode,
  Uri: { parse: (s: string) => s },
  editor: {
    setModelMarkers: () => {},
    getModelMarkers: () => [],
    onDidChangeMarkers: () => ({ dispose: () => {} }),
    getModel: (uri: string) => stub.models.get(uri) ?? null,
    getEditors: () => Array.from(stub.editors),
  },
};

export default function MonacoStub(props: EditorProps) {
  const model = modelFor(props.path ?? "default", props.defaultValue ?? "");
  const [, repaint] = useState(0);
  const area = useRef<HTMLTextAreaElement>(null);
  const latest = useRef(props);
  useEffect(() => { latest.current = props; });
  useEffect(() => {
    const changed = () => {
      repaint((n) => n + 1);
      latest.current.onChange?.(model.text, undefined as never);
    };
    model.listeners.add(changed);
    const editor = {
      getModel: () => model,
      // The element Monaco renders into, holding its input (`.inputarea`, here the textarea).
      getDomNode: () => area.current?.parentElement ?? null,
      addCommand: (k: number, cb: () => void) => { stub.commands.set(k, cb); },
      onDidChangeCursorPosition: () => ({ dispose: () => {} }),
      getPosition: () => null,
      setSelection: () => {},
      revealRangeInCenter: () => {},
      focus: () => {},
    };
    stub.editors.add(editor);
    latest.current.onMount?.(editor as never, monaco as never);
    return () => {
      model.listeners.delete(changed);
      stub.editors.delete(editor);
    };
  }, [model]);
  // As `@monaco-editor/react` 4.7 does on unmount: the model shown last is disposed unless
  // `keepCurrentModel`. A path change disposes nothing (the library leaves the old model).
  const shown = useRef(model);
  useEffect(() => { shown.current = model; }, [model]);
  useEffect(() => () => {
    if (!latest.current.keepCurrentModel) shown.current.dispose();
  }, []);
  return (
    <textarea
      ref={area}
      aria-label={props.options?.ariaLabel ?? "Flow source"}
      readOnly={!!props.options?.readOnly}
      value={model.text}
      onChange={(e) => typeInto(model, e.target.value)}
    />
  );
}

/**
 * A stand-in for `MonacoDiffEditor`: the two sides as read-only text boxes,
 * named "Original" and "Modified", with the props recorded in `stub.diffs`.
 */
export function MonacoDiffEditor(props: DiffEditorProps) {
  stub.diffs.push(props);
  return (
    <div role="group" aria-label={props.options?.ariaLabel ?? "Diff"}>
      <textarea aria-label="Original" readOnly value={props.original ?? ""} />
      <textarea aria-label="Modified" readOnly value={props.modified ?? ""} />
    </div>
  );
}
