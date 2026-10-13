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
 * literal (`queryKeys.ts`, whose strings are short words; `components/workflows/ReasonButton.tsx`, which has none) are NOT guarded here; a module added to
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
 *
 * THE TOPOLOGY'S OWN CHUNK (W7c Task 4). `components/workflows/topology/` brings `@xyflow/react`,
 * its stylesheet and `@dagrejs/dagre`, which neither the flow list nor the rest of the flow and
 * execution pages need: its two tabs (the editor's, W7c Task 5; the execution's, Task 6) reach it
 * through `lazy()` only. Markers flagged `topology` must therefore stay out of the entry, out of
 * the flow list's chunk, AND out of the chunks that carry the flow page (`FlowDetail.tsx`) and the
 * execution detail (`ExecutionDetail.tsx`), found by their own markers below.
 *
 * The editor's Topology tab (W7c Task 5) is registered with the other editor tabs
 * (`registerEditorTabs.tsx`, in the flow pages' chunk), but its body, `TopologyTab.tsx`, is reached
 * through `lazy()` only. W7c Task 6 added the execution's tab, `ExecutionTopologyTab.tsx`, which
 * `ExecutionDetail.tsx` reaches through `lazy()` only. Each
 * `topology` marker was proven by scratch builds (W7c task-4-report.md, "Chunk proof";
 * task-5-report.md for `TopologyTab.tsx`; task-6-report.md for the execution's tab).
 */
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { entryChunkName, entryPreloadNames } from "./dist-entry.mjs";

const DIST = "dist";
const LIST_MARKER = "egret.wf.ns-collapsed.";
/** The pages whose chunks must not carry a `topology` marker: their markers below. */
const TOPOLOGY_HOSTS = ["Changes since v", "Edits made to the flow since it ran are not used."];
const ASSETS = join(DIST, "assets");
/**
 * `[module, marker, flag?]`: the module the marker proves stayed lazy. `editor` modules must also
 * stay out of the flow list's chunk; `topology` modules out of the list's chunk and the flow and
 * execution pages' chunks too.
 */
const MARKERS = [
  // The prefix of the flow list's collapse-state key (`nsCollapseKey`). Also finds the list's chunk.
  ["src/routes/workflows/FlowsList.tsx", LIST_MARKER],
  // A Kestra state name; no egret module outside the area names Kestra's states.
  ["src/components/workflows/StateChip.tsx", "RESUBMITTED"],
  // `EMPTY_SEGMENT_LABEL`, this module's only string literal.
  ["src/components/workflows/namespaceTree.ts", "(empty segment)"],
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
  // The Revisions tab (W7a Task 7), on the flow page. Monaco's diff editor itself stays in the Monaco
  // chunk (`MonacoDiffEditor`, `assert-monaco-lazy.mjs`).
  ["src/routes/workflows/FlowRevisions.tsx", "The saved draft already has this text", "editor"],
  ["src/components/workflows/RevisionDiff.tsx", "Differences between ", "editor"],
  // The executions list (W7b Task 2), in the area's own chunk with the flow list; the flow page's
  // Executions tab imports it from there. Its filter-miss sentence.
  ["src/routes/workflows/ExecutionsList.tsx", "No executions match the current filters."],
  // A time preset's label. ("Last 30 days" is also a dashboard filter's, so not that one.)
  ["src/routes/workflows/executionsListModel.ts", "Last 24 hours"],
  // `ESTIMATED_DURATION_TITLE`, shared with the execution detail (Tasks 3 and 4).
  ["src/components/workflows/executions/timing.ts", "so its creation and last update stand in."],
  ["src/components/workflows/executions/trigger.tsx", "Loop iteration"],
  // `staleNote`'s sentence. Polling and the clock are shared by the list and the detail.
  ["src/components/workflows/executions/polling.ts", "it keeps trying on its own."],
  // The execution detail (W7b Task 3), a lazy chunk of its own like the flow pages, which the
  // gantt, logs and outputs (Tasks 4–6) grow; flagged to stay out of the list's chunk too.
  ["src/routes/workflows/ExecutionDetail.tsx", "Edits made to the flow since it ran are not used.", "editor"],
  ["src/routes/workflows/ExecutionOverview.tsx", "No trigger started this execution, so it has no trigger variables.", "editor"],
  // `REPLAY_NOT_FINISHED_REASON`.
  ["src/components/workflows/executions/actions.ts", "Replay is available when the execution has finished", "editor"],
  // The Gantt tab (W7b Task 4), in the execution detail's chunk. `markerReason`'s sentence, the
  // run panel's close label, and the unknown-`?run=` note.
  ["src/components/workflows/executions/gantt/ganttModel.ts", "Cancelled before it started.", "editor"],
  ["src/components/workflows/executions/gantt/GanttView.tsx", "Close the task run", "editor"],
  ["src/components/workflows/executions/gantt/ExecutionGanttTab.tsx", "The link may be from another execution.", "editor"],
  // The logs (W7b Task 5), in the execution detail's chunk: the cursor hook's refusal sentence, the
  // view's note above "Load earlier", and the Logs tab's unknown-`?run=` note. `logRows.ts` has no
  // literal of its own worth a marker (its one string is "execution") and is reached only through
  // `LogView.tsx`, so it is NOT guarded here.
  ["src/components/workflows/executions/logs/useLogFollow.ts", "The logs could not be read.", "editor"],
  ["src/components/workflows/executions/logs/LogView.tsx", "Older lines were set aside to keep the view fast.", "editor"],
  ["src/components/workflows/executions/logs/ExecutionLogsTab.tsx", "so every run is shown", "editor"],
  // Outputs and files (W7b Task 6), in the execution detail's chunk: `LOOP_ITERATION_NOTE`, the
  // tree's empty sentence, the tab's unknown-`?run=` note, `NO_LONGER_AVAILABLE`, the row's
  // unread-size sentence, and the drawer's not-previewable title.
  ["src/components/workflows/executions/outputs/outputsModel.ts", "The parent execution cannot reach these outputs.", "editor"],
  ["src/components/workflows/executions/outputs/OutputsTree.tsx", "This task run has no outputs.", "editor"],
  ["src/components/workflows/executions/outputs/ExecutionOutputsTab.tsx", "has no outputs in this execution.", "editor"],
  ["src/components/workflows/executions/files/fileModel.ts", "This file is no longer available", "editor"],
  ["src/components/workflows/executions/files/FileRow.tsx", "The size could not be read; the file may still open.", "editor"],
  ["src/components/workflows/executions/files/FilePreviewDrawer.tsx", "This file cannot be previewed", "editor"],
  // The topology (W7c Task 4): `ORIENTATION_KEY`; the list view's treegrid name; the context
  // guard's sentence; `REFUSAL_NOTE`. `TopologyEdges.tsx` has no literal worth a marker and is
  // reached only through `TopologyView.tsx`. `layout.ts` lost its literal (`"cluster:triggers"`) in
  // Task 4's fix round 1 and has none left; it is the only importer of `@dagrejs/dagre`, so a static
  // import of it pulls dagre along, and dagre's marker below goes where it goes (W7c Task 5 found
  // the stale marker when the flag turned the checks on).
  ["src/components/workflows/topology/TopologyView.tsx", "egret.workflows.topology.orientation", "topology"],
  ["src/components/workflows/topology/TopologyListView.tsx", "Tasks of the flow", "topology"],
  ["src/components/workflows/topology/TopologyNodes.tsx", "A topology node is drawn outside TopologyView.", "topology"],
  ["src/components/workflows/topology/topologyModel.ts", "egret does not run this task", "topology"],
  // The editor's Topology tab (W7c Task 5): its note for a node no longer in the YAML.
  ["src/components/workflows/topology/TopologyTab.tsx", "is not in the YAML as it is now.", "topology"],
  // The execution's Topology tab (W7c Task 6), reached from `ExecutionDetail.tsx` through `lazy()`
  // only: its note for a `?node=` the graph does not have, and the iteration list's empty
  // sentence. `selection.ts` has no literal worth a marker and is reached only through these two.
  ["src/components/workflows/executions/topology/ExecutionTopologyTab.tsx", "is not a node of this execution's graph.", "topology"],
  ["src/components/workflows/executions/topology/LoopIterations.tsx", "has not run in any iteration yet.", "topology"],
  // The two libraries: a class name of xyflow's viewport, and one of dagre's ranker names (which
  // also stands for `layout.ts`, see above).
  ["node_modules/@xyflow/react (via TopologyView.tsx)", "react-flow__viewport-portal", "topology"],
  ["node_modules/@dagrejs/dagre (via layout.ts)", "network-simplex", "topology"],
];

const entry = entryChunkName(DIST);
// The entry and the chunks it imports statically (preloaded by `index.html`): everything a page
// downloads before any `lazy()` runs. A marker in any of them is eager.
const eager = [entry, ...entryPreloadNames(DIST)];
const entrySource = eager.map((f) => readFileSync(join(ASSETS, f), "utf8")).join("\n");
const others = readdirSync(ASSETS).filter((f) => f.endsWith(".js") && !eager.includes(f));

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

/**
 * `chunk` and every chunk it imports statically, transitively: what the browser fetches with it.
 * A lazy `import("./x.js")` is not followed. Rollup moves a module two lazy chunks share (the flow
 * pages share `FlowEditPage.tsx` and the editor tabs) into a chunk of its own that both import
 * statically, so "not in the page's chunk" alone does not mean "not loaded with the page": W7c
 * Task 5's scratch static import of `TopologyTab.tsx` from `registerEditorTabs.tsx` put the whole
 * topology into `FlowEditPage-*.js`, and a check of `FlowDetail-*.js` alone passed it.
 */
function loadedWith(chunk) {
  const seen = new Set();
  const queue = [chunk];
  while (queue.length > 0) {
    const name = queue.pop();
    if (seen.has(name)) continue;
    seen.add(name);
    const source = readFileSync(join(ASSETS, name), "utf8");
    for (const m of source.matchAll(/\b(?:import|export)\s*(?:[^"'();]*?\bfrom\s*)?["']\.\/([^"']+\.js)["']/g)) queue.push(m[1]);
  }
  return [...seen].filter((f) => others.includes(f));
}
const listLoads = listChunk ? loadedWith(listChunk) : [];

const hostChunks = TOPOLOGY_HOSTS.map((marker) => {
  const found = others.filter((f) => readFileSync(join(ASSETS, f), "utf8").includes(marker));
  if (found.length !== 1) {
    console.error(`✗ ${marker} is in ${found.length} non-entry chunks (${found.join(", ") || "none"}); the topology's own-chunk check cannot run.`);
    failed = true;
  }
  return found[0];
});
const hostLoads = hostChunks.map((host) => (host ? loadedWith(host) : []));

for (const [module, marker, flag] of MARKERS) {
  const carriers = others.filter((f) => readFileSync(join(ASSETS, f), "utf8").includes(marker));
  if (flag === "topology") {
    for (const [i, host] of hostChunks.entries()) {
      const loaded = carriers.filter((f) => hostLoads[i].includes(f));
      if (loaded.length > 0) {
        console.error(
          `✗ ${marker} (${module}) loads with the flow page or the execution detail (${host}, in ${loaded.join(", ")}).\n` +
            `  The topology is its own chunk: its tabs reach it through lazy(), never a static import.`,
        );
        failed = true;
      }
    }
  }
  const withList = carriers.filter((f) => listLoads.includes(f));
  if (flag && withList.length > 0) {
    console.error(
      `✗ ${marker} (${module}) loads with the flow list's chunk (${listChunk}, in ${withList.join(", ")}).\n` +
        `  The flow editor now loads with the flow list. Look for a static import of it from\n` +
        `  FlowsList.tsx or WorkflowsRoutes.tsx; the flow pages are reached through lazy().`,
    );
    failed = true;
  }
  if (entrySource.includes(marker)) {
    console.error(
      `✗ ${marker} (${module}) is in the entry chunk (${entry}) or a chunk it preloads.\n` +
        `  That module is eager again: every page now downloads it, including the login screen.\n` +
        `  Look for a static import of it from a module the entry reaches (App.tsx, the layout,\n` +
        `  a shared helper).`,
    );
    failed = true;
  }
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
const topologyCount = MARKERS.filter((m) => m[2] === "topology").length;
console.log(
  `✓ The Workflows area stays out of the entry chunk (${entry}); ` +
    `${MARKERS.length} module markers found in lazy chunks.\n` +
    `✓ The flow editor stays out of the flow list's chunk (${listChunk}); ` +
    `${MARKERS.filter((m) => m[2] === "editor").length} editor markers checked.\n` +
    `✓ The topology stays in its own chunk: ${topologyCount} topology markers checked.`,
);
