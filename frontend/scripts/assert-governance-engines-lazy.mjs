#!/usr/bin/env node
/**
 * IS THE GOVERNANCE ENGINE & POLICY AREA OUT OF THE ENTRY CHUNK?
 *
 * `App.tsx` reaches the Engine & policy area (DG redesign #4a) only through
 * `lazy(() => import("@/routes/governance/engines/EnginesRoutes"))`
 * (FE-ROUTE-2). Neither vitest nor `vite dev` code-splits, so only a
 * production build can show the area stayed lazy; this follows
 * `assert-governance-overview-lazy.mjs`.
 *
 * One marker per guarded module: a string literal from that module that
 * survives minification and that nothing outside the area spells. Each must be
 * absent from the entry chunk and from the chunks the entry preloads, and
 * present in some other chunk (a marker found nowhere is stale and would let
 * this pass on any bundle). `engineUrl.ts` and `PolicyRedirect.tsx` are
 * statically imported on purpose (the old Policies URLs, the Overview and
 * Control Plane link into the area through them) and are not guarded.
 */
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { entryChunkName, entryPreloadNames } from "./dist-entry.mjs";

const DIST = "dist";
const ASSETS = join(DIST, "assets");
const MARKERS = [
  // The editor route of an unbound profile (Q2: the kind stays in the URL).
  ["src/routes/governance/engines/EnginesRoutes.tsx", "profiles/:service/advanced/policies/:kind/:policyId"],
  // The index's empty Engines section (spec §5).
  ["src/routes/governance/engines/EnginesIndex.tsx", "No engine is recorded. Engines are created in Control plane."],
  // The index's notice after an old URL named no profile.
  ["src/routes/governance/engines/MissingProfileNotice.tsx", "No policy profile named "],
  // The engine page's line for a reader without engine:read (EP19).
  ["src/routes/governance/engines/EnginePage.tsx", "Plugin state needs the engine:read permission."],
  // The profile page's header for an unbound profile (EP13).
  ["src/routes/governance/engines/ProfilePage.tsx", "No engine is bound to this profile."],
  // The shared-profile banner (EP14).
  ["src/routes/governance/engines/PageFrame.tsx", "Everything on this page applies to all of them."],
  // The Engine permissions tab on an engine with no profile (spec §6.1).
  ["src/routes/governance/engines/tabs/EnginePermissionsTab.tsx", "so nothing can be granted on it. Bind a profile in Control plane."],
  // The Engine permissions tab's skipped-compile report (Q4).
  ["src/routes/governance/engines/tabs/EnginePermissionsPanel.tsx", "Saved, but not compiled: "],
  // The grant dialog's principal check.
  ["src/routes/governance/engines/tabs/EngineGrantDialog.tsx", "Add at least one user, group or role."],
  // Check's Trino impersonation sentence (F11 §5.4).
  ["src/routes/governance/engines/tabs/CheckCapabilityDialog.tsx", "egret cannot decide impersonation on this engine"],
  // The value controls' fixed-key text.
  ["src/routes/governance/engines/tabs/capabilityValues.tsx", "Applies to all (*)"],
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
        `  Engine & policy is eager again. Look for a static import of anything under\n` +
        `  src/routes/governance/engines/ other than engineUrl.ts and PolicyRedirect.tsx.`,
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
console.log(`✓ Engine & policy stays out of the entry chunk (${entry}) and its preloads; markers found in lazy chunks.`);
