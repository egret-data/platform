#!/usr/bin/env node
/**
 * IS THE `.xlsx` WRITER IN ITS OWN CHUNK, AND IN A CHUNK AT ALL?
 *
 * `export-xlsx.ts` reaches `write-excel-file` through `await import(...)`, so Rollup should emit it
 * as a separate chunk that no dashboard fetches until someone picks `.xlsx`. One static import that
 * is USED — an editor auto-import, a "tidy-up" of the dynamic one — collapses that back into the
 * entry bundle, and NOTHING ELSE IN THIS REPO WOULD NOTICE:
 *
 * (Measured while proving this check can fail: a BARE `import "write-excel-file/browser"` does NOT
 * trip it, because the package declares `sideEffects: false` and Rollup deletes the statement
 * outright. Neither would a type-only import. The failure this guards against is a static import
 * whose binding is called — which is exactly the shape a "tidy-up" of the dynamic import takes, and
 * which moved the entry chunk from 3,562,208 to 3,634,579 bytes.)
 *
 *   NEITHER VITEST NOR `vite dev` TREE-SHAKES OR CODE-SPLITS. Every unit test passes with the
 *   library eager, lazy, or duplicated. The property this asserts exists only in a production
 *   build, so only a production build can be asked about it. This is the same reason
 *   `assert-label-layout-bundled.mjs` exists, and that one was written after the repo shipped a
 *   build where six files' options were dead code.
 *
 * WHY THIS MARKER. `writeXlsxFile`, `generateXlsxFileContents` and `zipToArrayBuffer` are local
 * bindings and are mangled to nothing by the minifier. `xl/worksheets/sheet` is a STRING LITERAL
 * the writer concatenates a sheet id onto (`xlsx/generateXlsxFileContents.js:118`), so it survives
 * minification verbatim and cannot be produced by anything else in this app.
 *
 * TWO ASSERTIONS, AND THE SECOND MATTERS AS MUCH AS THE FIRST. "Absent from the entry chunk" is
 * also true of a build where the import was deleted outright and `.xlsx` downloads throw — so the
 * marker must be absent from the entry AND present in exactly one other chunk.
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { entryChunkName } from "./dist-entry.mjs";

const DIST = join(dirname(fileURLToPath(import.meta.url)), "..", "dist");
const ASSETS = join(DIST, "assets");
const MARKER = "xl/worksheets/sheet";

function die(message) {
  console.error(`\n  ✗ ${message}\n`);
  process.exit(1);
}

let files;
try {
  files = readdirSync(ASSETS).filter((f) => f.endsWith(".js"));
} catch {
  die(`no ${ASSETS} — run \`npm run build\` first. This check reads the production bundle; there is nothing to assert without one.`);
}

// The entry is whatever `dist/index.html` loads, NOT whatever is called `index-*.js`. The chunk
// this script exists to find is itself named `index-<hash>.js`, because Rollup names a lazy chunk
// after its module file and write-excel-file's browser build is `browser/index.js`.
let entry;
try {
  entry = entryChunkName(DIST);
} catch (e) {
  die(e.message);
}
if (!files.includes(entry)) die(`dist/index.html loads /assets/${entry}, which is not in ${ASSETS} — \`dist/\` is inconsistent. Remove it and rebuild.`);

const carriers = files.filter((f) => readFileSync(join(ASSETS, f), "utf8").includes(MARKER));
const size = (f) => statSync(join(ASSETS, f)).size;

if (carriers.includes(entry)) {
  die(
    `${entry} (${size(entry)} bytes) contains "${MARKER}" — the .xlsx writer is in the ENTRY bundle.\n`
    + `    Every dashboard visitor now downloads write-excel-file + fflate to render a chart.\n`
    + `    Check that src/components/chart/export-xlsx.ts still reaches it only through \`await import(...)\`,\n`
    + `    and that nothing imports write-excel-file statically.`,
  );
}
if (carriers.length === 0) {
  die(
    `no chunk in dist/assets contains "${MARKER}" — the .xlsx writer is in NO bundle at all.\n`
    + `    A .xlsx download will throw at runtime. Check that downloadXlsx() is still reachable\n`
    + `    from ChartExportMenu, and that the import specifier is "write-excel-file/browser".`,
  );
}
if (carriers.length > 1) {
  die(`"${MARKER}" appears in ${carriers.length} chunks (${carriers.join(", ")}) — the writer is duplicated.`);
}

console.log(
  `  ✓ ${entry} (${size(entry)} bytes) carries no .xlsx writer; `
  + `write-excel-file is lazy in ${carriers[0]} (${size(carriers[0])} bytes)`,
);
