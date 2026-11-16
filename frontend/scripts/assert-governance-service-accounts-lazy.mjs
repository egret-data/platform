#!/usr/bin/env node
/**
 * IS THE GOVERNANCE SERVICE ACCOUNTS AREA OUT OF THE ENTRY CHUNK?
 *
 * `App.tsx` reaches the Service accounts tab (SA3: the index, one account's
 * page, and its credentials panel) only through
 * `lazy(() => import("@/routes/governance/service-accounts/ServiceAccountsRoutes"))`
 * (FE-ROUTE-2). Neither vitest nor `vite dev` code-splits, so only a
 * production build can show the area stayed lazy; this follows
 * `assert-governance-people-lazy.mjs`.
 *
 * One marker per guarded module: a string literal from that module that
 * survives minification and that nothing outside the area spells. Each must be
 * absent from the entry chunk and from the chunks the entry preloads, and
 * present in some other chunk. Modules without a literal of their own are
 * caught through the module that imports them.
 */
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { entryChunkName, entryPreloadNames } from "./dist-entry.mjs";

const DIST = "dist";
const ASSETS = join(DIST, "assets");
const MARKERS = [
  ["src/routes/governance/service-accounts/ServiceAccountsIndex.tsx", "Non-human principals that run jobs and call the API"],
  ["src/routes/governance/service-accounts/ServiceAccountDetail.tsx", "The service account could not be loaded."],
  ["src/routes/governance/service-accounts/CredentialsPanel.tsx", "Clients using this credential stop working at once."],
  ["src/routes/governance/service-accounts/CreateCredentialDialog.tsx", "Choose a warehouse and consumer type first"],
  ["src/routes/governance/service-accounts/credentialErrors.ts", "The credentials could not be loaded."],
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
        `  The Service accounts area is eager again. Look for a static import of anything under\n` +
        `  src/routes/governance/service-accounts/.`,
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
console.log(`✓ The Governance Service accounts area stays out of the entry chunk (${entry}) and its preloads; markers found in lazy chunks.`);
