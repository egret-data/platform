#!/usr/bin/env node
/**
 * IS THE GOVERNANCE PEOPLE AREA OUT OF THE ENTRY CHUNK?
 *
 * `App.tsx` reaches the People area (DG redesign #2: the People & groups index
 * and the principal pages) only through
 * `lazy(() => import("@/routes/governance/people/PeopleRoutes"))` (FE-ROUTE-2).
 * Neither vitest nor `vite dev` code-splits, so only a production build can
 * show the area stayed lazy; this follows `assert-governance-data-lazy.mjs`.
 *
 * One marker per guarded module: a string literal from that module that
 * survives minification and that nothing outside the area spells. Each must be
 * absent from the entry chunk and from the chunks the entry preloads, and
 * present in some other chunk (a marker found nowhere is stale and would let
 * this pass on any bundle). `PeopleRoutes.tsx` has no literal of its own; it
 * imports `PeopleIndex.tsx`, so a static import of it is caught through
 * PeopleIndex's marker. `PrincipalPage.tsx` is still a frame with no literal of
 * its own; the page built on it adds its marker here. `peopleUrl.ts` is NOT
 * guarded: pages outside the area link into it through it.
 */
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { entryChunkName, entryPreloadNames } from "./dist-entry.mjs";

const DIST = "dist";
const ASSETS = join(DIST, "assets");
const MARKERS = [
  // The index's description.
  ["src/routes/governance/people/PeopleIndex.tsx", "Users, groups and roles, and the egret grants and rules that reach each one."],
  // The Unmatched names tab's empty state.
  ["src/routes/governance/people/UnmatchedNames.tsx", "Every grant principal and rule exemption names someone egret knows."],
  // What a username nobody has says.
  ["src/routes/governance/people/ByNameRedirect.tsx", "No user has the username "],
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
        `  The People area is eager again. Look for a static import of anything under\n` +
        `  src/routes/governance/people/ other than peopleUrl.ts.`,
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
console.log(`✓ The Governance People area stays out of the entry chunk (${entry}) and its preloads; markers found in lazy chunks.`);
