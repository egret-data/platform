import { useEffect, useMemo, useRef, useState } from "react";
import type { Monaco, OnMount } from "@monaco-editor/react";
import type * as MonacoNS from "monaco-editor";
import { Lock } from "lucide-react";
import Editor from "@/components/monaco/MonacoEditor";
import type { EditorContext, EditorHandle } from "@/components/workflows/editor/EditorContext";
import { flowModelPath, rememberMonaco } from "@/components/workflows/editor/flowModel";
import { collectProblems, type SchemaMarker } from "@/components/workflows/editor/problems";
import { SchemaUnavailable } from "@/components/workflows/editor/SchemaUnavailable";
import type { YamlRange } from "@/components/workflows/editor/yamlPath";
import { useIsDark } from "@/hooks/useIsDark";

/** The owner of the markers server validation sets (spec §5.2); `monaco-yaml`'s are `yaml`. */
export const VALIDATE_OWNER = "egret-validate";

// `@monaco-editor/react`'s `Monaco` is loosely typed; the namespace's own types check the calls below.
type Mounted = { editor: MonacoNS.editor.IStandaloneCodeEditor; monaco: typeof MonacoNS };

const toMonaco = (r: YamlRange): MonacoNS.IRange => ({
  startLineNumber: r.startLine,
  startColumn: r.startCol,
  endLineNumber: r.endLine,
  endColumn: r.endCol,
});

/**
 * The Code tab (spec §5.2): Monaco on the flow's YAML, with `monaco-yaml`'s
 * completion, hover and schema errors, and the server's findings as markers.
 *
 * **It survives remounts.** The panel frame remounts it whenever the set of
 * panels changes (spec §5.1). The text lives in the context; the model, with
 * its undo history, lives in Monaco under one path per flow and is kept on
 * unmount (`keepCurrentModel`); cursor and scroll are saved by that path
 * (`saveViewState`). The page disposes the model when it leaves the flow
 * (`disposeFlowModel`).
 */
export function CodeTab({ ctx }: { ctx: EditorContext }) {
  const [mounted, setMounted] = useState<Mounted | null>(null);
  const [yamlFailed, setYamlFailed] = useState(false);
  const isDark = useIsDark();
  // Monaco's callbacks are bound once, at mount; they read the latest context through this.
  const latest = useRef(ctx);
  useEffect(() => { latest.current = ctx; });

  const handleMount: OnMount = (editor, monaco) => {
    rememberMonaco(monaco);
    // Registered while read-only too, and inert there: read-only can arrive after mount.
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => {
      if (!latest.current.readOnly) latest.current.save?.();
    });
    editor.onDidChangeCursorPosition(() => {
      const position = editor.getPosition();
      const model = editor.getModel();
      if (position && model) latest.current.setCursorOffset(model.getOffsetAt(position));
    });
    setMounted({ editor, monaco: monaco as typeof MonacoNS });
  };

  // The editor is not controlled. A `value` prop races the keyboard: `@monaco-editor/react`
  // writes `value` back whenever it differs from the model, and when a key lands between
  // `onChange` and the render it caused, that write is one keystroke old and the key is lost.
  // Measured in Chromium on this tab: typing `  - id: t3` at Playwright's default speed kept
  // `  - d ` in three runs out of three. So the model is the source of truth while typing, and
  // `ctx.source` is written into it only when it changed from outside (a reload, a revision).
  // `emitted` holds what this editor reported, so an echo arriving late is not taken for that.
  const emitted = useRef(new Set<string>());
  const onChange = (v: string | undefined) => {
    const text = v ?? "";
    if (emitted.current.size > 100) emitted.current.clear();
    emitted.current.add(text);
    ctx.setSource(text);
  };
  useEffect(() => {
    if (!mounted) return;
    const model = mounted.editor.getModel();
    if (!model) return;
    if (model.getValue() === ctx.source) {
      emitted.current.clear();
      return;
    }
    if (emitted.current.has(ctx.source)) return;
    // Replaced from outside: one undoable edit, which works on a read-only editor too.
    model.pushStackElement();
    model.pushEditOperations([], [{ range: model.getFullModelRange(), text: ctx.source }], () => null);
    model.pushStackElement();
    emitted.current.clear();
  }, [mounted, ctx.source]);

  // The bridge: Problems moves the cursor through it.
  const { bridge } = ctx;
  useEffect(() => {
    if (!mounted) return;
    const { editor } = mounted;
    const handle: EditorHandle = {
      reveal(range) {
        const r = toMonaco(range);
        editor.setSelection(r);
        editor.revealRangeInCenter(r);
        editor.focus();
      },
    };
    bridge.attach(handle);
    return () => bridge.detach(handle);
  }, [mounted, bridge]);

  // The server's findings, placed against the current text.
  const serverProblems = useMemo(
    () => collectProblems(ctx.source, ctx.validation.findings, []),
    [ctx.source, ctx.validation.findings],
  );
  useEffect(() => {
    if (!mounted) return;
    const { editor, monaco } = mounted;
    const model = editor.getModel();
    if (!model) return;
    monaco.editor.setModelMarkers(
      model,
      VALIDATE_OWNER,
      serverProblems.map((p) => ({
        severity: p.severity === "error" ? monaco.MarkerSeverity.Error : monaco.MarkerSeverity.Warning,
        message: p.mapped || !p.path ? p.message : `${p.message} (${p.path})`,
        ...toMonaco(p.range),
      })),
    );
  }, [mounted, serverProblems]);

  // The schema's markers, which `monaco-yaml` sets, reported for the Problems tab.
  useEffect(() => {
    if (!mounted) return;
    const { editor, monaco } = mounted;
    // Setting our own markers fires this event too, and reporting re-renders the page, which may
    // set them again: report only a change, or the two would chase each other forever.
    let reported = "";
    const sync = () => {
      const model = editor.getModel();
      if (!model) return;
      const markers: SchemaMarker[] = monaco.editor
        .getModelMarkers({ resource: model.uri })
        .filter((m) => m.owner !== VALIDATE_OWNER)
        .filter((m) => m.severity === monaco.MarkerSeverity.Error || m.severity === monaco.MarkerSeverity.Warning)
        .map((m) => ({
          severity: m.severity === monaco.MarkerSeverity.Error ? "error" : "warning",
          message: m.message,
          range: { startLine: m.startLineNumber, startCol: m.startColumn, endLine: m.endLineNumber, endCol: m.endColumn },
        }));
      const key = JSON.stringify(markers);
      if (key === reported) return;
      reported = key;
      latest.current.setSchemaMarkers(markers);
    };
    sync();
    const sub = monaco.editor.onDidChangeMarkers((uris) => {
      const model = editor.getModel();
      if (!uris || !model || uris.some((u) => String(u) === String(model.uri))) sync();
    });
    return () => sub.dispose();
  }, [mounted]);

  // `monaco-yaml`, from its own lazy chunk, once per schema ETag.
  const { status, schema, etag } = ctx.schema;
  const { workspace } = ctx;
  useEffect(() => {
    if (!mounted || status === "loading") return;
    let cancelled = false;
    import("@/components/workflows/editor/monacoYaml").then(
      ({ configureFlowYaml }) => {
        if (cancelled) return;
        configureFlowYaml(mounted.monaco as Monaco, status === "ready"
          ? { schema, etag, workspace }
          : { schema: null, etag: null, workspace });
      },
      (err: unknown) => {
        console.error("monaco-yaml failed to load:", err);
        if (!cancelled) setYamlFailed(true);
      },
    );
    return () => { cancelled = true; };
  }, [mounted, status, schema, etag, workspace]);

  return (
    <div className="flex h-full flex-col">
      {ctx.readOnly && (
        <div role="note" className="flex items-center gap-2 border-b border-border bg-muted/40 px-3 py-2 text-sm text-muted-foreground">
          <Lock aria-hidden className="size-4 shrink-0" />
          Read only. Editing a flow needs the workflow:manage permission.
        </div>
      )}
      {ctx.schema.status === "error" && <SchemaUnavailable error={ctx.schema.error} retry={ctx.schema.retry} />}
      {yamlFailed && (
        <div role="alert" className="border-b border-border bg-muted/40 px-3 py-2 text-sm text-muted-foreground">
          Autocomplete unavailable: the YAML support could not be loaded. Reload the page to try again.
        </div>
      )}
      <div className="min-h-0 flex-1">
        <Editor
          height="100%"
          path={flowModelPath(ctx.modelKey)}
          defaultLanguage="yaml"
          keepCurrentModel
          saveViewState
          defaultValue={ctx.source}
          onChange={onChange}
          onMount={handleMount}
          theme={isDark ? "vs-dark" : "vs"}
          options={{
            minimap: { enabled: false },
            fontSize: 13,
            tabSize: 2,
            insertSpaces: true,
            scrollBeyondLastLine: false,
            automaticLayout: true,
            readOnly: ctx.readOnly,
            // `readOnly` alone leaves the textarea writable to assistive tech (see `SqlEditor`).
            domReadOnly: ctx.readOnly,
            ariaLabel: "Flow source",
          }}
        />
      </div>
    </div>
  );
}
