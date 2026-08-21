import { loader } from "@monaco-editor/react";
import * as monaco from "monaco-editor/editor";
import EditorWorker from "monaco-editor/editor/editor.worker?worker";

// Editor features (find/replace, folding, bracket matching, hover, context menu,
// multi-cursor, …). `features/register.all` is the package's own list, but it
// stops short of nine contributions that the full `monaco-editor` entry also
// pulls in — including the suggest *controller*, without which a registered
// completion provider never shows a widget. Diff this block against
// `monaco-editor/esm/vs/index.js` on every Monaco upgrade.
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

// Only the two languages egret edits: SQL (SQL Lab, Data API Query) and Python
// (notebook cells). The full entry registers ~80 more plus the TypeScript, CSS,
// HTML and JSON language services, which cost ~14MB of dist for nothing here.
import "monaco-editor/languages/definitions/sql/register";
import "monaco-editor/languages/definitions/python/register";

self.MonacoEnvironment = { getWorker: () => new EditorWorker() };

loader.config({ monaco });
