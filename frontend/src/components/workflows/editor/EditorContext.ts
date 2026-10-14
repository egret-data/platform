import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { FilesHost } from "@/components/workflows/editor/fileTabs";
import type { FlowSchemaState } from "@/components/workflows/editor/useFlowSchema";
import { useFlowSchema } from "@/components/workflows/editor/useFlowSchema";
import type { ServerValidation } from "@/components/workflows/editor/useServerValidation";
import { useServerValidation } from "@/components/workflows/editor/useServerValidation";
import { collectProblems, type Problem, type SchemaMarker } from "@/components/workflows/editor/problems";
import { createYamlView, parseYaml, type YamlDoc, type YamlRange, type YamlView } from "@/components/workflows/editor/yamlPath";

/** What the Code tab exposes so another tab can move its cursor. */
export type EditorHandle = { reveal: (range: YamlRange) => void };

/**
 * The link between the tabs and the one mounted editor. The Code tab attaches
 * while it is mounted; a range asked for while it is not (it is being
 * remounted, or another tab of its panel is selected) waits and is applied
 * when it attaches.
 */
export type EditorBridge = {
  attach: (handle: EditorHandle) => void;
  detach: (handle: EditorHandle) => void;
  focus: (range: YamlRange) => void;
};

export function createEditorBridge(): EditorBridge {
  let current: EditorHandle | null = null;
  let pending: YamlRange | null = null;
  return {
    attach(handle) {
      current = handle;
      if (pending) {
        handle.reveal(pending);
        pending = null;
      }
    },
    detach(handle) {
      if (current === handle) current = null;
    },
    focus(range) {
      if (current) current.reveal(range);
      else pending = range;
    },
  };
}

/**
 * The state the Code, Docs and Problems tabs share: the `context` the flow
 * page hands `<MultiPanel>`, passed to every tab's `render`.
 *
 * The frame remounts a tab whenever the set of panels changes (spec §5.1,
 * Mounting), so nothing a tab must keep lives in the tab. The source text is
 * here; Monaco's model, undo history and view state live in Monaco under the
 * flow's model path (`flowModel.ts`), which the Code tab never disposes.
 */
export type EditorContext = {
  workspace: string;
  /** The flow's uuid, or `"new"` on the create page; names the Monaco model. */
  modelKey: string;
  source: string;
  setSource: (source: string) => void;
  /** No `workflow:manage`: the editor is read-only and the server is not asked to validate. */
  readOnly: boolean;
  validation: ServerValidation;
  schema: FlowSchemaState;
  /** The one parse of `source`, with the lookups the tabs need (Docs' `typeAt`). */
  yaml: YamlView;
  /**
   * The server's findings and the schema's markers, placed in `source` and
   * sorted (`collectProblems`). Computed once per change, here; the Code
   * tab's markers, the Problems tab and its badge all read this.
   */
  problems: Problem[];
  /** What `monaco-yaml` marked from the schema; the Code tab reports them. */
  schemaMarkers: SchemaMarker[];
  setSchemaMarkers: (markers: SchemaMarker[]) => void;
  /** The cursor's offset in `source`, 150 ms behind the editor (the Docs tab follows it). */
  cursorOffset: number | null;
  setCursorOffset: (offset: number) => void;
  /** Move the editor's cursor to `range` and focus it. */
  focusRange: (range: YamlRange) => void;
  bridge: EditorBridge;
  /** Ctrl/Cmd+S. Ignored while read-only. */
  save?: () => void;
  /**
   * The flow's namespace files (W7d spec §4.3): the Files tab and the
   * `file:<path>` tabs. Absent on the create page, which has no namespace yet.
   */
  files?: FilesHost;
};

export const CURSOR_DEBOUNCE_MS = 150;

type Options = {
  workspace: string;
  /** Undefined on the create page: validation then uses the id-less route. */
  flowId?: string;
  initialSource: string;
  readOnly: boolean;
  save?: () => void;
  /** The flow's namespace files, from `useFilesHost`; the create page has none. */
  files?: FilesHost;
  /**
   * Called when a tab asks for the editor (a Problems item was selected), so
   * the page can select the Code tab if another tab of its panel is showing.
   */
  onRevealCode?: () => void;
};

/**
 * Builds the {@link EditorContext} for one flow. The page calls it once and
 * hands the result to `<MultiPanel context>`; the page also disposes the
 * flow's model when it leaves the flow (`disposeFlowModel`).
 */
export function useFlowEditor({ workspace, flowId, initialSource, readOnly, save, files, onRevealCode }: Options): EditorContext {
  const [source, setSource] = useState(initialSource);
  const [schemaMarkers, setSchemaMarkers] = useState<SchemaMarker[]>([]);
  const [cursorOffset, setCursorNow] = useState<number | null>(null);
  const schema = useFlowSchema(workspace);
  const validation = useServerValidation(workspace, source, flowId, { enabled: !readOnly });
  const [bridge] = useState(createEditorBridge);

  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const setCursorOffset = useCallback((offset: number) => {
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setCursorNow(offset), CURSOR_DEBOUNCE_MS);
  }, []);

  const reveal = useRef(onRevealCode);
  useEffect(() => { reveal.current = onRevealCode; });
  const focusRange = useCallback((range: YamlRange) => {
    reveal.current?.();
    bridge.focus(range);
  }, [bridge]);

  const yaml = useYamlView(source);
  const problems = useMemo(
    () => collectProblems(yaml, validation.findings, schemaMarkers),
    [yaml, validation.findings, schemaMarkers],
  );

  const modelKey = flowId ?? "new";
  return useMemo(
    () => ({
      workspace, modelKey, source, setSource, readOnly, validation, schema, yaml, problems,
      schemaMarkers, setSchemaMarkers, cursorOffset, setCursorOffset, focusRange, bridge, save, files,
    }),
    [workspace, modelKey, source, readOnly, validation, schema, yaml, problems, schemaMarkers, cursorOffset, setCursorOffset, focusRange, bridge, save, files],
  );
}

/**
 * One parse per source, and the last source that parsed, for the lookups made
 * while the text is half-typed (`createYamlView`). The last good parse is kept
 * as state updated during render, React's pattern for state derived from the
 * previous render.
 */
export function useYamlView(source: string): YamlView {
  const doc = useMemo(() => parseYaml(source), [source]);
  const [lastGood, setLastGood] = useState<YamlDoc | null>(null);
  if (doc.ok && lastGood !== doc) setLastGood(doc);
  return useMemo(() => createYamlView(doc, doc.ok ? null : lastGood), [doc, lastGood]);
}
