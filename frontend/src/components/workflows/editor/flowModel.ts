import type { Monaco } from "@monaco-editor/react";

/**
 * The Monaco model of one flow's source. One stable path per flow keeps one
 * model, with its undo history, across every remount of the Code tab
 * (`keepCurrentModel`), and `@monaco-editor/react` keys the saved view state
 * (cursor, selection, scroll) by the same path (`saveViewState`).
 *
 * `monaco-yaml` attaches the flow schema to the models matching
 * {@link FLOW_MODEL_GLOB}, so every flow path must stay under it.
 */
export const FLOW_MODEL_GLOB = "inmemory://egret/flow/*.yaml";

/** `key` is the flow's uuid, or `"new"` on the create page. */
export const flowModelPath = (key: string) => `inmemory://egret/flow/${encodeURIComponent(key)}.yaml`;

/** The Monaco namespace, once the lazy chunk has loaded; the Code tab records it on mount. */
let loaded: Monaco | null = null;

export function rememberMonaco(monaco: Monaco): void {
  loaded = monaco;
}

/**
 * Dispose a flow's model. The flow page calls this when it leaves the flow:
 * the Code tab keeps the model on unmount on purpose, so nothing else would.
 * Does nothing when Monaco never loaded or the model does not exist.
 */
export function disposeFlowModel(key: string): void {
  if (!loaded) return;
  loaded.editor.getModel(loaded.Uri.parse(flowModelPath(key)))?.dispose();
}
