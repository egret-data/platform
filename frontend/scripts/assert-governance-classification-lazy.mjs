#!/usr/bin/env node
/**
 * IS THE GOVERNANCE CLASSIFICATION AREA OUT OF THE ENTRY CHUNK?
 *
 * `App.tsx` reaches the Classification area (DG redesign #4b) only through
 * `lazy(() => import("@/routes/governance/classifications/ClassificationsRoutes"))`
 * (FE-ROUTE-2). Neither vitest nor `vite dev` code-splits, so only a
 * production build can show the area stayed lazy; this follows
 * `assert-governance-engines-lazy.mjs`.
 *
 * One marker per guarded module: a string literal from that module that
 * survives minification and that nothing outside the area spells. Each must be
 * absent from the entry chunk and from the chunks the entry preloads, and
 * present in some other chunk (a marker found nowhere is stale and would let
 * this pass on any bundle). `tagUrl.ts` is statically imported on purpose
 * (Engine & policy and People link into the area through it) and is not
 * guarded. `ClassificationsRoutes.tsx` (the route table) holds no literal of
 * its own; it imports every page, so it cannot reach the entry chunk without
 * their markers going with it. `tagParts.tsx` (swatch, counts, the clearance
 * cell) holds only words other modules also spell; only guarded modules
 * import it.
 */
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { entryChunkName, entryPreloadNames } from "./dist-entry.mjs";

const DIST = "dist";
const ASSETS = join(DIST, "assets");
const MARKERS = [
  // The index's empty state (spec §3).
  ["src/routes/governance/classifications/ClassificationsIndex.tsx", "No classification yet."],
  // The tag page's not-found state (spec §5).
  ["src/routes/governance/classifications/TagPage.tsx", "No classification with this id."],
  // The header's save toast.
  ["src/routes/governance/classifications/TagHeader.tsx", "Classification saved."],
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
        `  Classification is eager again. Look for a static import of anything under\n` +
        `  src/routes/governance/classifications/ other than tagUrl.ts.`,
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
console.log(`✓ Classification stays out of the entry chunk (${entry}) and its preloads; markers found in lazy chunks.`);
