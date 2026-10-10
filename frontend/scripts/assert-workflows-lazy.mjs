#!/usr/bin/env node
/**
 * IS THE WORKFLOWS AREA OUT OF THE ENTRY CHUNK?
 *
 * `App.tsx` reaches the Workflows area only through
 * `lazy(() => import("@/routes/workflows/WorkflowsRoutes"))`, the first lazy route area in the app
 * (FE-ROUTE-2). Rollup then emits the area as its own chunk, fetched the first time a user opens a
 * Workflows page. Every later W7 package grows that area (the editor, Monaco's YAML support, the
 * execution views), and all of it would land in the chunk the login screen downloads the moment
 * anything in the entry graph imports one of its modules statically.
 *
 *   NEITHER VITEST NOR `vite dev` CODE-SPLITS. Every unit test passes with the area eager or lazy.
 *   The property exists only in a production build, so only a production build can be asked about
 *   it — the same reason `assert-monaco-lazy.mjs` exists, which this script follows.
 *
 * WHAT IT MEASURES, AND NO MORE. Rollup moves a module into the entry chunk on its own when the
 * entry graph imports it, while its lazy importers stay lazy -- so one marker proves only that ITS
 * module stayed out. There is one marker per guarded module below: a string literal from that
 * module that survives minification and that nothing outside the area spells. A static import of
 * `WorkflowsRoutes.tsx` is caught through `FlowsList.tsx`, which it imports. Modules with no such
 * literal (`queryKeys.ts`, whose strings are short words) are NOT guarded here; a module added to
 * the area gets a marker in this list or is not covered either.
 *
 * Each marker was proven by a scratch build that imported its module eagerly from `App.tsx`; the
 * run failed naming that marker (task-2-report.md, "Fix round 1").
 *
 * GUARDING THE GUARD. A marker that stops appearing anywhere would make this script pass forever
 * on any bundle, so it also asserts the marker IS present in some non-entry chunk. Renaming the key
 * fails this loudly instead of letting it go quiet.
 *
 * THE FLOW LIST'S OWN CHUNK (W7a Task 5). Inside the area, the flow pages (`FlowDetail.tsx`,
 * `FlowNew.tsx`) are a second `lazy()` split: they bring the multi-panel frame, the editor tabs and
 * the `yaml` library, which the flow list never uses. The markers flagged `editor` must therefore
 * also stay out of the chunk that carries the flow list (found by its own marker). A static import
 * of an editor module from the list, or of `registerEditorTabs` from `WorkflowsRoutes.tsx` (where it
 * lived until Task 5), pulls them in and fails here. Each `editor` marker was proven by a scratch
 * build that imported its module eagerly from `WorkflowsRoutes.tsx` (task-5-report.md).
 */
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { entryChunkName } from "./dist-entry.mjs";

const DIST = "dist";
const LIST_MARKER = "egret.wf.ns-collapsed.";
const ASSETS = join(DIST, "assets");
/**
 * `[module, marker, editor?]`: the module the marker proves stayed lazy. `editor` modules must also
 * stay out of the flow list's chunk.
 */
const MARKERS = [
  // The prefix of the flow list's collapse-state key (`nsCollapseKey`). Also finds the list's chunk.
  ["src/routes/workflows/FlowsList.tsx", LIST_MARKER],
  // A Kestra state name; no egret module outside the area names Kestra's states.
  ["src/components/workflows/StateChip.tsx", "RESUBMITTED"],
  // `EMPTY_SEGMENT_LABEL`, this module's only string literal.
  ["src/components/workflows/namespaceTree.ts", "(empty segment)"],
  ["src/routes/workflows/ExecutionsPlaceholder.tsx", "The executions list is not here yet"],
  // The flow editor's tabs (W7a Task 4), registered by the flow pages' chunk (Task 5).
  ["src/components/workflows/editor/CodeTab.tsx", "egret-validate", "editor"],
  ["src/components/workflows/editor/DocsTab.tsx", "This type has no documented outputs.", "editor"],
  ["src/components/workflows/editor/ProblemsTab.tsx", "(not found in the source, shown on line 1)", "editor"],
  // The multi-panel frame (W7a Task 3), which nothing imported before the flow pages.
  ["src/components/workflows/panels/MultiPanel.tsx", "Not over a drop target.", "editor"],
  // `LAYOUT_KEY_PREFIX`.
  ["src/components/workflows/panels/usePersistedLayout.ts", "egret.wf.layout.", "editor"],
  // The `yaml` library, through `yamlPath.ts`: one of its own error codes. It is also in
  // `monaco-yaml`'s worker file, which is a non-entry chunk and harmless.
  ["node_modules/yaml (via src/components/workflows/editor/yamlPath.ts)", "BLOCK_AS_IMPLICIT_KEY", "editor"],
  // The flow pages (W7a Task 5).
  ["src/routes/workflows/FlowDetail.tsx", "Changes since v", "editor"],
  // The create page's template.
  ["src/routes/workflows/FlowNew.tsx", "Hello from egret", "editor"],
  ["src/components/workflows/PublishDialog.tsx", "What changed in this version", "editor"],
  ["src/components/workflows/StaleSaveDialog.tsx", "Discard and reload", "editor"],
  ["src/components/workflows/draftState.ts", "Save first: Run uses the saved draft", "editor"],
  // The run form (W7a Task 6), opened from the flow page.
  ["src/components/workflows/run/RunFlow.tsx", "Fix the YAML, save, then run it.", "editor"],
  ["src/components/workflows/run/inputControls.tsx", "This input depends on other inputs", "editor"],
  ["src/components/workflows/run/inputModel.ts", "Add at least one item.", "editor"],
  // The minimal execution page (W7a Task 6), in the area's own chunk with the list.
  ["src/routes/workflows/ExecutionMinimal.tsx", "The execution ended before any task ran."],
];

const entry = entryChunkName(DIST);
const entrySource = readFileSync(join(ASSETS, entry), "utf8");
const others = readdirSync(ASSETS).filter((f) => f.endsWith(".js") && f !== entry);

let failed = false;

const listChunks = others.filter((f) => readFileSync(join(ASSETS, f), "utf8").includes(LIST_MARKER));
if (listChunks.length !== 1) {
  console.error(
    `✗ The flow list's chunk is not one chunk: ${LIST_MARKER} is in ${listChunks.length} non-entry chunks ` +
      `(${listChunks.join(", ") || "none"}). The list-chunk checks below cannot run.`,
  );
  failed = true;
}
const listChunk = listChunks.length === 1 ? listChunks[0] : null;
const listSource = listChunk ? readFileSync(join(ASSETS, listChunk), "utf8") : "";

for (const [module, marker, editor] of MARKERS) {
  if (editor && listChunk && listSource.includes(marker)) {
    console.error(
      `✗ ${marker} (${module}) is in the flow list's chunk (${listChunk}).\n` +
        `  The flow editor now loads with the flow list. Look for a static import of it from\n` +
        `  FlowsList.tsx or WorkflowsRoutes.tsx; the flow pages are reached through lazy().`,
    );
    failed = true;
  }
  if (entrySource.includes(marker)) {
    console.error(
      `✗ ${marker} (${module}) is in the entry chunk (${entry}).\n` +
        `  That module is eager again: every page now downloads it, including the login screen.\n` +
        `  Look for a static import of it from a module the entry reaches (App.tsx, the layout,\n` +
        `  a shared helper).`,
    );
    failed = true;
  }
  const carriers = others.filter((f) => readFileSync(join(ASSETS, f), "utf8").includes(marker));
  // Only stale when it is in NO chunk: an eager area has its marker in the entry
  // and in no lazy chunk, and the message above already names that failure.
  if (carriers.length === 0 && !entrySource.includes(marker)) {
    console.error(
      `✗ ${marker} appears in NO chunk at all.\n` +
        `  The marker is stale, so this guard would pass on any bundle. Find the literal in\n` +
        `  ${module} and update MARKERS to match.`,
    );
    failed = true;
  }
}

if (failed) process.exit(1);
console.log(
  `✓ The Workflows area stays out of the entry chunk (${entry}); ${MARKERS.length} module markers found in lazy chunks.\n` +
    `✓ The flow editor stays out of the flow list's chunk (${listChunk}); ` +
    `${MARKERS.filter((m) => m[2]).length} editor markers checked.`,
);
