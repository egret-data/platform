/**
 * The Monaco bundle. Everything heavy in this module is here on purpose: it is
 * reached ONLY through the `import()` in `components/monaco/MonacoEditor.tsx`,
 * so Rolldown gives it its own chunk and a user who never opens an editor never
 * downloads it. Import it statically from anywhere and that property is gone —
 * `__tests__/monaco.test.ts` fails if you do.
 *
 * `@monaco-editor/loader` defaults to
 * `https://cdn.jsdelivr.net/npm/monaco-editor@<v>/min/vs` and only stops using
 * it if `loader.config({ monaco })` runs before the first `loader.init()`.
 * Module evaluation is strictly before the lazy component can render, and
 * `init()` is called on mount, so the ordering here is deterministic.
 */
import ReactEditor, { loader } from "@monaco-editor/react";
import * as monaco from "monaco-editor/editor";
import EditorWorker from "monaco-editor/editor/editor.worker?worker";

// Editor features (find/replace, folding, bracket matching, hover, context menu,
// multi-cursor, …). `features/register.all` is the package's own list, but it
// stops short of nine contributions that the full `monaco-editor` entry also
// pulls in — including the suggest *controller*, without which a registered
// completion provider never shows a widget. `__tests__/monaco.test.ts`
// re-derives that diff from monaco's own files, so an upgrade cannot lose one
// quietly.
import "monaco-editor/features/register.all";
import "monaco-editor/editor/browser/coreCommands";
import "monaco-editor/editor/contrib/caretOperations/browser/caretOperations";
import "monaco-editor/editor/contrib/dropOrPasteInto/browser/copyPasteContribution";
import "monaco-editor/editor/contrib/find/browser/findController";
import "monaco-editor/editor/contrib/gotoSymbol/browser/goToCommands";
import "monaco-editor/editor/contrib/gotoError/browser/markerSelectionStatus";
import "monaco-editor/editor/contrib/semanticTokens/browser/documentSemanticTokens";
import "monaco-editor/editor/contrib/suggest/browser/suggestController";
import "monaco-editor/editor/common/standaloneStrings";

// Only the two languages egret edits: SQL (SQL Lab, Data API Query, dataset
// detail) and Python (Data API Query/response scripting). The full entry
// registers ~80 more plus the TypeScript, CSS, HTML and JSON language
// services, which cost ~10MB of dist for nothing here.
import "monaco-editor/languages/definitions/sql/register";
import "monaco-editor/languages/definitions/python/register";

self.MonacoEnvironment = { getWorker: () => new EditorWorker() };

loader.config({ monaco });

/** `React.lazy` resolves a module's default export, so this IS the component
 *  `MonacoEditor` renders once the chunk has landed. */
export default ReactEditor;
