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
 * import it. `tabs/ViewsTab.tsx` only passes the tag to `ViewExposureSection`,
 * which lives outside the area.
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
  // The delete dialog's rules line without policy:read (CL14).
  ["src/routes/governance/classifications/DeleteTagDialog.tsx", "Rules: needs policy:read to count."],
  // Where used's Data empty sentence (spec §5.1).
  ["src/routes/governance/classifications/tabs/WhereUsedTab.tsx", "No table or column carries this classification."],
  // The Clearance tab's compile line (spec §5.2).
  ["src/routes/governance/classifications/tabs/ClearanceTab.tsx", "Compiles to one deny tag policy in each Trino profile"],
  // The Mask & filter tab without policy:read (spec §5.3).
  ["src/routes/governance/classifications/tabs/RulesTab.tsx", "Rules are read with the policy:read permission"],
  // The Hand-written tab's empty state (spec §5.5).
  ["src/routes/governance/classifications/tabs/HandWrittenTagTab.tsx", "No hand-written tag policy names this classification."],
];

/**
 * The shared hand-written policy views (`src/routes/governance/handwritten/`,
 * #4b spec §6): Data and People import them, and so does this area's
 * Hand-written tab. Each marker must stay out of the entry chunk and its
 * preloads, and its chunk must be loaded (statically, from the area's own
 * chunk) by each area named: the area's anchor is a marker of that area.
 */
const AREA_ANCHORS = {
  Data: "Reading grants needs the policy:read permission, which this account does not have.",
  People: "Users, groups and roles, and the egret grants and rules that reach each one.",
  Classification: "No hand-written tag policy names this classification.",
};
const HANDWRITTEN = [
  // Route A's section (Data).
  ["src/routes/governance/handwritten/HandWrittenSection.tsx", "No hand-written policy reaches this object.", ["Data"]],
  // Route B's section (People).
  ["src/routes/governance/handwritten/PrincipalHandWrittenSection.tsx", "No hand-written policy names this principal.", ["People"]],
  // The shared words: Q1's sentence.
  ["src/routes/governance/handwritten/handwrittenText.ts", "Engines resolve no groups, so on the engine it reaches nobody.", ["Data", "People", "Classification"]],
  // The item summary's empty item.
  ["src/routes/governance/handwritten/HandWrittenItems.tsx", "nobody named", ["Data", "Classification"]],
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

/** The chunks `chunk` imports statically (not `import()`), transitively, itself included. */
function staticClosure(chunk) {
  const seen = new Set();
  const walk = (f) => {
    if (seen.has(f)) return;
    seen.add(f);
    for (const m of read(f).matchAll(/\bimport\s*(?:[\w*{}\s,$]+?\s*from\s*)?["']\.\/([^"']+?\.js)["']/g)) walk(m[1]);
  };
  walk(chunk);
  return seen;
}

for (const [module, marker, areas] of HANDWRITTEN) {
  const carriers = chunks.filter((f) => read(f).includes(marker));
  const eagerCarriers = carriers.filter((f) => eager.has(f));
  if (eagerCarriers.length > 0) {
    console.error(`✗ ${module} is in a chunk the entry loads (${eagerCarriers.join(", ")}). Look for a static import of src/routes/governance/handwritten/ from the entry graph.`);
    failed = true;
    continue;
  }
  if (carriers.length === 0) {
    console.error(`✗ "${marker}" (${module}) appears in NO chunk at all. The marker is stale.`);
    failed = true;
    continue;
  }
  for (const area of areas) {
    const anchors = chunks.filter((f) => read(f).includes(AREA_ANCHORS[area]));
    if (anchors.length === 0) {
      console.error(`✗ the ${area} anchor "${AREA_ANCHORS[area]}" appears in no chunk; update AREA_ANCHORS.`);
      failed = true;
      continue;
    }
    const reached = anchors.some((a) => carriers.some((c) => staticClosure(a).has(c)));
    if (!reached) {
      console.error(`✗ ${module} is not loaded by the ${area} area's chunk (${anchors.join(", ")}).`);
      failed = true;
    }
  }
}

if (failed) process.exit(1);
console.log(`✓ Classification stays out of the entry chunk (${entry}) and its preloads; markers found in lazy chunks; the hand-written views load with Data, People and Classification.`);
