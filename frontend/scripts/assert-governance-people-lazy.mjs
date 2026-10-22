#!/usr/bin/env node
/**
 * IS THE GOVERNANCE PEOPLE AREA OUT OF THE ENTRY CHUNK?
 *
 * `App.tsx` reaches the People area (DG redesign #2: the People & groups index,
 * the principal pages, and one grant's and one rule's pages) only through
 * `lazy(() => import("@/routes/governance/people/PeopleRoutes"))`,
 * `lazy(() => import(".../people/GrantPage"))` and
 * `lazy(() => import(".../people/RulePage"))` (FE-ROUTE-2).
 * Neither vitest nor `vite dev` code-splits, so only a production build can
 * show the area stayed lazy; this follows `assert-governance-data-lazy.mjs`.
 *
 * One marker per guarded module: a string literal from that module that
 * survives minification and that nothing outside the area spells. Each must be
 * absent from the entry chunk and from the chunks the entry preloads, and
 * present in some other chunk (a marker found nowhere is stale and would let
 * this pass on any bundle). `PeopleRoutes.tsx` has no literal of its own; it
 * imports `PeopleIndex.tsx`, so a static import of it is caught through
 * PeopleIndex's marker; `principalData.ts` has none either, and PrincipalPage's
 * marker catches it, as it catches `tabs/CheckAccessTab.tsx`, whose one sentence
 * `components/perm/MembersExplainView.tsx` (shared, outside the area) also spells; `itemParts.tsx`
 * has none, and GrantPage's marker catches it. `peopleUrl.ts` is NOT guarded: pages outside the area
 * link into the area through it.
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
  // `Missing`: a group or role no one has.
  ["src/routes/governance/people/PrincipalPage.tsx", ". Grants naming it match nobody; see "],
  // `NO_USERNAME_SENTENCE`.
  ["src/routes/governance/people/PrincipalHeader.tsx", "egret allocates one at their first query, notebook or engine token"],
  // A group member with no username.
  ["src/routes/governance/people/tabs/OverviewTab.tsx", " allocates one at compile"],
  // A role or workspace a group holds, in "How it reaches".
  ["src/routes/governance/people/tabs/PermissionsTab.tsx", ", held by group "],
  // `otherRulesText`.
  ["src/routes/governance/people/tabs/MasksFiltersTab.tsx", " other rules apply to everyone they do not exempt; "],
  // `GROUPS_ONLY_GATE`.
  ["src/routes/governance/people/tabs/ClearancesTab.tsx", "Engines do not enforce this gate (it names only groups)"],
  // `CHECK_ACCESS_INTRO`.
  ["src/routes/governance/people/CheckAccessDialog.tsx", "can reach a target, as each engine that serves it"],
  // The prefix a uuid ref may carry.
  ["src/routes/governance/people/reach.ts", "urn:uuid:"],
  // `ASSIGNMENT_READ_ONLY`. GrantPage and RulePage are lazy imports of their own
  // in App.tsx (`/governance/grants/:id`, `/governance/rules/:id`).
  ["src/routes/governance/people/GrantPage.tsx", "The workspace assignment sync writes this grant; "],
  // `RULE_NEEDS_UPDATE`.
  ["src/routes/governance/people/RulePage.tsx", "Editing or deleting a rule needs the policy:update permission."],
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
