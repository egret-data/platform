import { useEffect, useRef } from "react";
import type { DiffOnMount, MonacoDiffEditor as DiffEditorInstance } from "@monaco-editor/react";
import { MonacoDiffEditor } from "@/components/monaco/MonacoEditor";
import { useIsDark } from "@/hooks/useIsDark";

/** One side of a comparison: what it is ("v2", "Current draft") and its YAML. */
export type DiffSide = { label: string; source: string };

/**
 * Two flow sources side by side in Monaco's diff editor (spec §5, Revisions),
 * read-only, the older text on the left. Monaco switches to an inline diff by
 * itself when the panel is too narrow for two columns.
 *
 * **Models.** `@monaco-editor/react` 4.7 disposes the two text models before
 * the diff editor on unmount, and Monaco refuses that ("TextModel got disposed
 * before DiffEditorWidget model got reset"). So the component keeps its
 * models (`keepCurrent…Model`), records them on mount, and disposes them in
 * its own unmount cleanup, deferred one task (`setTimeout(…, 0)`) so that the
 * library's cleanup has disposed the widget first. Not `onDidDispose`: monaco
 * 0.56's diff editor registers that event and never fires it
 * (`delegatingEditorImpl.js`), so a listener there disposed nothing, and every
 * mount leaked two models. A new pair is written into the same two models
 * (`@monaco-editor/react` sets `original` and `modified` on change). Measured
 * in Chromium (W7a Task 7, fix round 1): `monaco.editor.getModels()` returns
 * to its count before the mount after each unmount, with no console error.
 * No model path is given: a path under `inmemory://egret/flow/` would get
 * `monaco-yaml`'s schema markers, which a frozen version has no use for.
 */
export function RevisionDiff({ left, right }: { left: DiffSide; right: DiffSide }) {
  const isDark = useIsDark();
  const models = useRef<ReturnType<DiffEditorInstance["getModel"]>>(null);
  const onMount: DiffOnMount = (editor) => {
    models.current = editor.getModel();
  };
  useEffect(
    () => () => {
      const mine = models.current;
      models.current = null;
      if (!mine) return;
      // After the library's own cleanup has disposed the widget (see above).
      setTimeout(() => {
        mine.original.dispose();
        mine.modified.dispose();
      }, 0);
    },
    [],
  );
  const same = left.source === right.source;
  return (
    <div className="flex h-full min-h-0 flex-col gap-2">
      <div className="grid grid-cols-2 gap-2 text-sm font-medium" data-testid="revision-diff-labels">
        <span>{left.label}</span>
        <span>{right.label}</span>
      </div>
      {same && (
        <p role="status" className="text-sm text-muted-foreground">
          {left.label} and {right.label} have the same text.
        </p>
      )}
      <div className="min-h-0 flex-1 rounded border border-border">
        <MonacoDiffEditor
          height="100%"
          language="yaml"
          original={left.source}
          modified={right.source}
          keepCurrentOriginalModel
          keepCurrentModifiedModel
          onMount={onMount}
          theme={isDark ? "vs-dark" : "vs"}
          options={{
            readOnly: true,
            originalEditable: false,
            domReadOnly: true,
            renderSideBySide: true,
            minimap: { enabled: false },
            fontSize: 13,
            scrollBeyondLastLine: false,
            automaticLayout: true,
            ariaLabel: `Differences between ${left.label} and ${right.label}`,
          }}
        />
      </div>
    </div>
  );
}
