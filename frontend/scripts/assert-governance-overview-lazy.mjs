#!/usr/bin/env node
/**
 * IS THE GOVERNANCE OVERVIEW OUT OF THE ENTRY CHUNK?
 *
 * `App.tsx` reaches the Overview (DG redesign #3, the Governance index) only
 * through `lazy(() => import("@/routes/governance/overview/OverviewPage"))`
 * (FE-ROUTE-2). Neither vitest nor `vite dev` code-splits, so only a
 * production build can show the area stayed lazy; this follows
 * `assert-governance-people-lazy.mjs`.
 *
 * One marker per guarded module: a string literal from that module that
 * survives minification and that nothing outside the area spells. Each must be
 * absent from the entry chunk and from the chunks the entry preloads, and
 * present in some other chunk (a marker found nowhere is stale and would let
 * this pass on any bundle). `refUrl.ts` and `AttentionRow.tsx` have no literal
 * of their own that nothing else spells; `AttentionSection.tsx`, which imports
 * both, carries them.
 */
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { entryChunkName, entryPreloadNames } from "./dist-entry.mjs";

const DIST = "dist";
const ASSETS = join(DIST, "assets");
const MARKERS = [
  // The page's description.
  ["src/routes/governance/overview/OverviewPage.tsx", "What needs attention in governance, and what changed recently."],
  // The qualified empty state.
  ["src/routes/governance/overview/AttentionSection.tsx", "Nothing needs attention among the checks you can run."],
  // The strip's test id.
  ["src/routes/governance/overview/CountsStrip.tsx", "overview-counts"],
];

const entry = entryChunkName(DIST);
const eager = new Set([entry, ...entryPreloadNames(DIST)]);
const read = (f) => readFileSync(join(ASSETS, f), "utf8");
const chunks = readdirSync(ASSETS).filter((f) => f.endsWith(".js"));

let failed = false;
for (const [module, marker] of MARKERS) {
  const carriers = chunks.filter((f) => read(f).includes(marker));
  const eagerCarriers = carriers.filter((f) => eager.has(f));
  if (eagerCarriers.length > 0) {
    console.error(
      `✗ ${module} is in a chunk the entry loads (${eagerCarriers.join(", ")}).\n` +
        `  The Overview is eager again. Look for a static import of anything under\n` +
        `  src/routes/governance/overview/.`,
    );
    failed = true;
  } else if (carriers.length === 0) {
    console.error(
      `✗ "${marker}" (${module}) appears in NO chunk at all.\n` +
        `  The marker is stale, so this guard would pass on any bundle. Pick a literal the module still has.`,
    );
    failed = true;
  }
}

if (failed) process.exit(1);
console.log(`✓ The Governance Overview stays out of the entry chunk (${entry}) and its preloads; markers found in lazy chunks.`);
