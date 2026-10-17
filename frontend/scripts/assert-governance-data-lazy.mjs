#!/usr/bin/env node
/**
 * IS THE GOVERNANCE DATA AREA OUT OF THE ENTRY CHUNK?
 *
 * `App.tsx` reaches the Data area (DG redesign #1: the catalog tree and the object pages) only
 * through `lazy(() => import("@/routes/governance/data/DataRoutes"))` (FE-ROUTE-2). Neither vitest
 * nor `vite dev` code-splits, so only a production build can show the area stayed lazy; this
 * follows `assert-workflows-lazy.mjs`.
 *
 * One marker per guarded module: a string literal from that module that survives minification and
 * that nothing outside the area spells. Each must be absent from the entry chunk and from the
 * chunks the entry preloads, and present in some other chunk (a marker found nowhere is stale and
 * would let this pass on any bundle). `DataRoutes.tsx` has no literal of its own; it imports
 * `DataPage.tsx`, so a static import of it is caught through DataPage's marker. `objectPath.ts` is
 * NOT guarded: the old Governance list pages link into the area through it.
 */
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { entryChunkName, entryPreloadNames } from "./dist-entry.mjs";

const DIST = "dist";
const ASSETS = join(DIST, "assets");
const MARKERS = [
  // `LEVEL_FAILED`.
  ["src/routes/governance/data/CatalogTree.tsx", "This level could not be listed. Retry, or reload the page."],
  // `DataIndex`'s description.
  ["src/routes/governance/data/DataPage.tsx", "Open a catalog, schema or table from the tree"],
  // `NO_READERS`.
  ["src/routes/governance/data/tabs/LineageTab.tsx", "No view or materialized view reads this table"],
  // `TAGS_FAILED`.
  ["src/routes/governance/data/ObjectHeader.tsx", "Tags could not be loaded."],
  // The Masks & filters tab's label.
  ["src/routes/governance/data/ObjectTabs.tsx", "Masks & filters"],
  // What a view without recorded facts says.
  ["src/routes/governance/data/tabs/OverviewTab.tsx", "egret has no security mode or lineage recorded for this object."],
  // A catalog with no warehouse.
  ["src/routes/governance/data/tabs/DetailsTab.tsx", "None: not an egret catalog"],
  // `GRANTS_NEED_POLICY_READ`.
  ["src/routes/governance/data/tabs/PermissionsTab.tsx", "Reading grants needs the policy:read permission"],
  // `RULES_NEED_POLICY_READ`.
  ["src/routes/governance/data/tabs/MasksFiltersTab.tsx", "Reading rules needs the policy:read permission"],
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
        `  The Data area is eager again. Look for a static import of anything under\n` +
        `  src/routes/governance/data/ other than objectPath.ts.`,
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
console.log(`✓ The Governance Data area stays out of the entry chunk (${entry}) and its preloads; markers found in lazy chunks.`);
