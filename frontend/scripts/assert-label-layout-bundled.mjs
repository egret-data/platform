#!/usr/bin/env node
/**
 * THE ONLY CHECK THAT CAN FAIL FOR THE RIGHT REASON: is ECharts' LabelManager in the shipped bundle?
 *
 * `labelLayout: { hideOverlap: true }` is set by six option builders and read by a component
 * echarts installs itself, as a top-level `use(installLabelLayout)` in `lib/export/core.js`. That
 * file is not in the package's `sideEffects` allowlist (`index*.js`, `lib/echarts.js`,
 * `lib/chart/*`, `lib/component/*`, `extension/**`, `theme/*`, `i18n/*`), so Rollup deletes the
 * call and the whole of `LabelManager.js` with it. `register.ts` therefore lists `LabelLayout`
 * explicitly. That listing is one import away from being deleted as redundant by anyone reading
 * echarts' docs, and nothing else in this repo would notice:
 *
 *   NEITHER VITEST NOR `vite dev` TREE-SHAKES. Every unit test, including the SSR ones that render
 *   through real ECharts, runs the backstop whether or not it is registered. The defect exists only
 *   in a production build, so only a production build can be asked about it.
 *
 * WHY THIS IS A CI STEP AND NOT A VITEST CASE. It reads `dist/`, which `vitest run` does not
 * produce. Inside the unit suite it would have to either fail on every machine that has not built
 * (breaking the normal test loop) or skip when `dist/` is absent — and a check that skips itself
 * when the artefact is missing is green on a machine that has never built one, which is precisely
 * the "passes for the wrong reason" failure this whole change is about. CI already runs
 * `npm run build`; this runs immediately after it, against the artefact that build just produced.
 *
 * WHY THESE TWO SYMBOLS AND NOT THE OBVIOUS ONES. `installLabelLayout`, `LabelManager` and
 * `shiftLayoutOnXY` are local bindings and are MANGLED to nothing by the minifier — measured: they
 * return zero hits in a bundle that DOES contain the component, so asserting on them would pass a
 * broken build and fail a fixed one for the wrong reason. `labelGuideIgnore` and `hostRect` are
 * property names on objects the layout writes and reads across module boundaries, so they survive
 * verbatim. Measured on this repo: 0 hits each without the registration, 2 hits each with it.
 */
import { readFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { entryChunkName } from "./dist-entry.mjs";

const DIST = join(dirname(fileURLToPath(import.meta.url)), "..", "dist");
const ASSETS = join(DIST, "assets");

/** Property names, not local bindings — see this file's header for why the obvious symbols cannot
 *  be used. */
const REQUIRED = ["labelGuideIgnore", "hostRect"];

function die(message) {
  console.error(`\n  ✗ ${message}\n`);
  process.exit(1);
}

// The entry is whatever `dist/index.html` loads. This used to be `/^index-.*\.js$/`, which stopped
// meaning "entry" the moment a lazy chunk was split out of a dependency whose module file is also
// called `index.js` — write-excel-file's is, so that regex found two candidates and this guard
// died on a freshly-built bundle. See `dist-entry.mjs`.
let entry;
try {
  entry = entryChunkName(DIST);
} catch (e) {
  die(e.message);
}

let source;
try {
  source = readFileSync(join(ASSETS, entry), "utf8");
} catch {
  die(`dist/index.html loads /assets/${entry}, which is not in ${ASSETS} — \`dist/\` is inconsistent. Remove it and rebuild.`);
}
const counts = Object.fromEntries(REQUIRED.map((s) => [s, source.split(s).length - 1]));
const missing = REQUIRED.filter((s) => counts[s] === 0);

if (missing.length > 0) {
  die(
    `${entry} contains no LabelManager: ${missing.join(", ")} absent `
    + `(${REQUIRED.map((s) => `${s}=${counts[s]}`).join(", ")}).\n`
    + `    Every \`labelLayout\` option in src/components/chart/echarts/options/ is dead in this build.\n`
    + `    Check that src/components/chart/echarts/register.ts still passes LabelLayout to use().`,
  );
}

console.log(`  ✓ ${entry} carries the label-overlap backstop (${REQUIRED.map((s) => `${s}×${counts[s]}`).join(", ")})`);
