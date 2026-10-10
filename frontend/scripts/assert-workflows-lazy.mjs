#!/usr/bin/env node
/**
 * IS THE WORKFLOWS AREA OUT OF THE ENTRY CHUNK?
 *
 * `App.tsx` reaches the Workflows area only through
 * `lazy(() => import("@/routes/workflows/WorkflowsRoutes"))`, the first lazy route area in the app
 * (FE-ROUTE-2). Rollup then emits the area as its own chunk, fetched the first time a user opens a
 * Workflows page. Every later W7 package grows that area (the editor, Monaco's YAML support, the
 * execution views), and all of it would land in the chunk the login screen downloads the moment
 * anything in the entry graph imports one of its modules statically: a `Sidebar` that imports a
 * state chip, an `App.tsx` that names a page, a shared helper that re-exports `namespaceTree`.
 *
 *   NEITHER VITEST NOR `vite dev` CODE-SPLITS. Every unit test passes with the area eager or lazy.
 *   The property exists only in a production build, so only a production build can be asked about
 *   it — the same reason `assert-monaco-lazy.mjs` exists, which this script follows.
 *
 * WHY THIS MARKER. `egret.wf.ns-collapsed.` is the prefix of the flow list's collapse-state
 * `localStorage` key (`nsCollapseKey` in `src/routes/workflows/FlowsList.tsx`). It is a string
 * literal, so minification keeps it, and nothing outside the Workflows area spells it.
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
const MARKERS = ["egret.wf.ns-collapsed."];

const entry = entryChunkName(DIST);
const entrySource = readFileSync(join(ASSETS, entry), "utf8");
const others = readdirSync(ASSETS).filter((f) => f.endsWith(".js") && f !== entry);

let failed = false;

for (const marker of MARKERS) {
  if (entrySource.includes(marker)) {
    console.error(
      `✗ ${marker} is in the entry chunk (${entry}).\n` +
        `  The Workflows area is eager again: every page now downloads it, including the login\n` +
        `  screen. Look for a static import of anything under src/routes/workflows/ or\n` +
        `  src/components/workflows/ from a module the entry reaches (App.tsx, the layout, a\n` +
        `  shared helper).`,
    );
    failed = true;
  }
  const carriers = others.filter((f) => readFileSync(join(ASSETS, f), "utf8").includes(marker));
  // Only stale when it is in NO chunk: an eager area has its marker in the entry
  // and in no lazy chunk, and the message above already names that failure.
  if (carriers.length === 0 && !entrySource.includes(marker)) {
    console.error(
      `✗ ${marker} appears in NO chunk at all.\n` +
        `  The marker is stale, so this guard would pass on any bundle. Check nsCollapseKey in\n` +
        `  src/routes/workflows/FlowsList.tsx and update MARKERS to match.`,
    );
    failed = true;
  }
}

if (failed) process.exit(1);
console.log(`✓ The Workflows area stays out of the entry chunk (${entry}); marker found in a lazy chunk.`);
