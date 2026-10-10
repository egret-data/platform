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
 */
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { entryChunkName } from "./dist-entry.mjs";

const DIST = "dist";
const ASSETS = join(DIST, "assets");
/** `[module, marker]`: the module the marker proves stayed lazy. */
const MARKERS = [
  // The prefix of the flow list's collapse-state key (`nsCollapseKey`).
  ["src/routes/workflows/FlowsList.tsx", "egret.wf.ns-collapsed."],
  // A Kestra state name; no egret module outside the area names Kestra's states.
  ["src/components/workflows/StateChip.tsx", "RESUBMITTED"],
  // `EMPTY_SEGMENT_LABEL`, this module's only string literal.
  ["src/components/workflows/namespaceTree.ts", "(empty segment)"],
  ["src/routes/workflows/ExecutionsPlaceholder.tsx", "The executions list is not here yet"],
  // The flow editor's tabs (W7a Task 4), registered when the area loads.
  ["src/components/workflows/editor/CodeTab.tsx", "egret-validate"],
  ["src/components/workflows/editor/DocsTab.tsx", "This type has no documented outputs."],
  ["src/components/workflows/editor/ProblemsTab.tsx", "(not found in the source, shown on line 1)"],
];

const entry = entryChunkName(DIST);
const entrySource = readFileSync(join(ASSETS, entry), "utf8");
const others = readdirSync(ASSETS).filter((f) => f.endsWith(".js") && f !== entry);

let failed = false;

for (const [module, marker] of MARKERS) {
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
  `✓ The Workflows area stays out of the entry chunk (${entry}); ${MARKERS.length} module markers found in lazy chunks.`,
);
